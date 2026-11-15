//! User panels written in Luau. A panel returns a table with a `render`
//! function; rendering yields the same native component tree as session
//! visuals, plus native app slots. Panels read a frozen snapshot of the app
//! and ask for changes through queued `host` actions, which the app checks
//! and runs after each call. Programs have no filesystem, network, process,
//! module loading or timers.
use crate::visual::Node;
use mlua::{
    chunk::ChunkMode, Function, Lua, LuaOptions, StdLib, Table, Value as LuaValue, VmState,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Number, Value};
use std::{
    cell::Cell,
    path::{Path, PathBuf},
    rc::Rc,
    time::{Duration, Instant},
};

mod hooks;
#[cfg(test)]
mod tests;
pub use hooks::*;

pub const MAX_SOURCE_BYTES: usize = 256 * 1024;
pub const MEMORY_LIMIT_BYTES: usize = 32 * 1024 * 1024;
/// Panels run on the UI thread, so a call may not hold it longer than this.
/// Tests run hundreds of suites in parallel and allow more wall time.
const CALL_DEADLINE: Duration = Duration::from_millis(if cfg!(test) { 2000 } else { 60 });
const MAX_DEPTH: usize = 32;
const MAX_VALUES: usize = 100_000;
const MAX_ACTIONS: usize = 32;
pub(crate) const SDK: &str = include_str!("panel/sdk.luau");
/// The shortest periodic refresh a panel can ask for.
pub const MIN_REFRESH: Duration = Duration::from_secs(1);

/// What a panel declares about itself when it loads.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PanelMeta {
    pub title: Option<String>,
    pub icon: Option<String>,
    pub refresh: Option<Duration>,
    pub placement: Placement,
}

/// Where the app shows a panel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Placement {
    /// A tab in the right panel.
    #[default]
    Right,
    /// Below the composer of a new, empty session.
    Composer,
}
impl Placement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Composer => "composer",
        }
    }
}

/// A change a panel asks the app to make. The app decides whether to run it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PanelAction {
    Notify { message: String },
    OpenFile { path: String },
    NewChat { prompt: String },
    Send { session: i64, message: String },
    OpenSession { session: i64 },
    Copy { text: String },
    SelectTab { tab: String },
}
impl PanelAction {
    /// Whether the action can make an agent do work. Untrusted project panels
    /// may not run these.
    pub fn starts_work(&self) -> bool {
        matches!(self, Self::NewChat { .. } | Self::Send { .. })
    }
}

/// A sandboxed Luau VM with the shared limits: the safe standard library,
/// no `require` or `loadstring`, read-only globals, a memory cap and a
/// per-call deadline.
pub(crate) struct Sandbox {
    pub lua: Lua,
    deadline: Rc<Cell<Instant>>,
    interrupted: Rc<Cell<bool>>,
}
impl Sandbox {
    /// Runs `prelude` before sandboxing and returns what it evaluates to.
    pub fn new(prelude: &str, name: &str) -> Result<(Self, Table), String> {
        // Luau's `os` has only clock, date, difftime and time.
        let libs = StdLib::MATH
            | StdLib::OS
            | StdLib::STRING
            | StdLib::TABLE
            | StdLib::UTF8
            | StdLib::BIT
            | StdLib::BUFFER
            | StdLib::VECTOR;
        let lua = Lua::new_with(libs, LuaOptions::default()).map_err(|e| e.to_string())?;
        lua.set_memory_limit(MEMORY_LIMIT_BYTES)
            .map_err(|e| e.to_string())?;
        let globals = lua.globals();
        for name in [
            "require",
            "loadstring",
            "getfenv",
            "setfenv",
            "collectgarbage",
        ] {
            globals
                .raw_set(name, LuaValue::Nil)
                .map_err(|e| e.to_string())?;
        }
        let deadline = Rc::new(Cell::new(Instant::now() + CALL_DEADLINE));
        let interrupted = Rc::new(Cell::new(false));
        let (watch, flag) = (deadline.clone(), interrupted.clone());
        lua.set_interrupt(move |_| {
            if Instant::now() >= watch.get() {
                flag.set(true);
                return Err(mlua::Error::runtime("time budget exceeded"));
            }
            Ok(VmState::Continue)
        });
        let sandbox = Self {
            lua,
            deadline,
            interrupted,
        };
        let controller: Table = sandbox.guard(name, || {
            sandbox
                .lua
                .load(prelude)
                .set_name("=sdk")
                .set_mode(ChunkMode::Text)
                .eval()
        })?;
        sandbox.lua.sandbox(true).map_err(|e| e.to_string())?;
        Ok((sandbox, controller))
    }
    /// Runs `f` under the deadline and turns its error into a message.
    pub fn guard<R>(&self, name: &str, f: impl FnOnce() -> mlua::Result<R>) -> Result<R, String> {
        self.deadline.set(Instant::now() + CALL_DEADLINE);
        self.interrupted.set(false);
        let result = f();
        // Nothing outside a call may be interrupted.
        self.deadline
            .set(Instant::now() + Duration::from_secs(3600 * 24 * 365));
        result.map_err(|e| {
            if self.interrupted.get() {
                format!(
                    "{name} exceeded its {} ms time budget",
                    CALL_DEADLINE.as_millis()
                )
            } else {
                error_message(name, &e)
            }
        })
    }
    /// Loads a program's source as a text chunk and runs it in a fresh
    /// environment holding the SDK's `ui` and `host`, over the frozen base.
    pub fn load(&self, name: &str, source: &str, controller: &Table) -> Result<LuaValue, String> {
        if source.trim().is_empty() || source.len() > MAX_SOURCE_BYTES {
            return Err(format!(
                "{name} must contain 1–{} KiB of source",
                MAX_SOURCE_BYTES / 1024
            ));
        }
        let e = |e: mlua::Error| e.to_string();
        let env = self.lua.create_table().map_err(e)?;
        for key in ["ui", "host"] {
            env.raw_set(key, controller.get::<Table>(key).map_err(e)?)
                .map_err(e)?;
        }
        let fallback = self.lua.create_table().map_err(e)?;
        fallback.raw_set("__index", self.lua.globals()).map_err(e)?;
        env.set_metatable(Some(fallback)).map_err(e)?;
        self.guard(name, || {
            self.lua
                .load(source)
                .set_name(format!("={name}"))
                .set_mode(ChunkMode::Text)
                .set_environment(env)
                .eval()
        })
    }
}

fn error_message(name: &str, error: &mlua::Error) -> String {
    let message = match error {
        mlua::Error::CallbackError { cause, .. } => return error_message(name, cause),
        mlua::Error::RuntimeError(m) | mlua::Error::SyntaxError { message: m, .. } => m.clone(),
        mlua::Error::MemoryError(_) => format!(
            "out of memory (limit {} MiB)",
            MEMORY_LIMIT_BYTES / 1024 / 1024
        ),
        other => other.to_string(),
    };
    // Luau prefixes errors with the chunk name already.
    let message: String = message.chars().take(2048).collect();
    if message.starts_with(name) {
        message
    } else {
        format!("{name}: {message}")
    }
}

/// One loaded panel program. Keep it alive to keep its state.
pub struct PanelRuntime {
    name: String,
    sandbox: Sandbox,
    controller: Table,
    meta: PanelMeta,
    current: Option<Node>,
    app: Option<(Value, Table)>,
}

impl PanelRuntime {
    /// Loads a panel. `name` labels its errors, usually the file name.
    pub fn new(name: &str, source: &str) -> Result<Self, String> {
        let (sandbox, controller) = Sandbox::new(SDK, name)?;
        let program = sandbox.load(name, source, &controller)?;
        let init: Function = controller.get("init").map_err(|e| e.to_string())?;
        let meta: Table = sandbox.guard(name, || init.call(program))?;
        let title: Option<String> = meta.get("title").map_err(|e| e.to_string())?;
        let icon: Option<String> = meta.get("icon").map_err(|e| e.to_string())?;
        let refresh: Option<f64> = meta.get("refresh").map_err(|e| e.to_string())?;
        let placement: Option<String> = meta.get("placement").map_err(|e| e.to_string())?;
        let meta = PanelMeta {
            placement: match placement.as_deref() {
                Some("composer") => Placement::Composer,
                _ => Placement::Right,
            },
            title: title
                .map(|t| t.trim().chars().take(48).collect::<String>())
                .filter(|t| !t.is_empty()),
            icon: icon.filter(|i| {
                !i.is_empty()
                    && i.len() <= 48
                    && i.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-')
            }),
            refresh: refresh
                .filter(|s| s.is_finite() && *s > 0.)
                .map(|s| Duration::from_secs_f64(s.min(86_400.)).max(MIN_REFRESH)),
        };
        Ok(Self {
            name: name.to_string(),
            sandbox,
            controller,
            meta,
            current: None,
            app: None,
        })
    }
    pub fn meta(&self) -> &PanelMeta {
        &self.meta
    }
    pub fn current(&self) -> Option<&Node> {
        self.current.as_ref()
    }

    /// Renders with the latest app snapshot.
    pub fn render(&mut self, app: &Value) -> Result<Node, String> {
        let app = self.app_table(app)?;
        let render: Function = self.controller.get("render").map_err(|e| e.to_string())?;
        let tree: LuaValue = self.sandbox.guard(&self.name, || render.call(app))?;
        self.accept(tree)
    }

    /// Runs a control's handler, checked against the control in the current
    /// render, then renders again.
    pub fn dispatch(&mut self, handler: &str, value: Value, app: &Value) -> Result<Node, String> {
        if handler.len() > 64 {
            return Err("Invalid panel event handler".into());
        }
        self.current
            .as_ref()
            .and_then(|node| crate::visual::validate_event(node, handler, &value))
            .unwrap_or_else(|| Err("This panel control is stale; use the current view".into()))?;
        let app = self.app_table(app)?;
        let payload = to_lua(&self.sandbox.lua, &value, 0)?;
        let dispatch: Function = self.controller.get("dispatch").map_err(|e| e.to_string())?;
        let tree: LuaValue = self
            .sandbox
            .guard(&self.name, || dispatch.call((handler, payload, app)))?;
        self.accept(tree)
    }

    /// Delivers an app event. Returns the new tree when the panel handled it.
    pub fn event(
        &mut self,
        name: &str,
        payload: &Value,
        app: &Value,
    ) -> Result<Option<Node>, String> {
        let app_table = self.app_table(app)?;
        let payload = to_lua(&self.sandbox.lua, payload, 0)?;
        let event: Function = self.controller.get("event").map_err(|e| e.to_string())?;
        let handled: bool = self
            .sandbox
            .guard(&self.name, || event.call((name, payload, app_table)))?;
        if !handled {
            return Ok(None);
        }
        self.render(app).map(Some)
    }

    /// The actions queued since the last call. Malformed ones are dropped.
    pub fn take_actions(&mut self) -> Vec<PanelAction> {
        take_actions(&self.sandbox, &self.controller, &self.name)
    }

    fn accept(&mut self, tree: LuaValue) -> Result<Node, String> {
        let json = from_lua(&tree).map_err(|e| format!("{}: {e}", self.name))?;
        let node: Node = serde_json::from_value(json)
            .map_err(|e| format!("{}: invalid component tree: {e}", self.name))?;
        node.validate_panel()
            .map_err(|e| format!("{}: {e}", self.name))?;
        self.current = Some(node.clone());
        Ok(node)
    }

    /// The snapshot as frozen Luau tables, rebuilt only when it changed.
    fn app_table(&mut self, app: &Value) -> Result<Table, String> {
        if let Some((last, table)) = &self.app {
            if last == app {
                return Ok(table.clone());
            }
        }
        let LuaValue::Table(table) = to_lua(&self.sandbox.lua, app, 0)? else {
            return Err("The app snapshot must be an object".into());
        };
        self.app = Some((app.clone(), table.clone()));
        Ok(table)
    }
}

pub(crate) fn take_actions(sandbox: &Sandbox, controller: &Table, name: &str) -> Vec<PanelAction> {
    let Ok(take) = controller.get::<Function>("takeActions") else {
        return Vec::new();
    };
    let Ok(list) = sandbox.guard(name, || take.call::<LuaValue>(())) else {
        return Vec::new();
    };
    let Ok(Value::Array(items)) = from_lua(&list) else {
        return Vec::new();
    };
    items
        .into_iter()
        .take(MAX_ACTIONS)
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect()
}

/// JSON to Luau: objects and arrays become frozen tables, arrays 1-based.
pub(crate) fn to_lua(lua: &Lua, value: &Value, depth: usize) -> Result<LuaValue, String> {
    if depth > MAX_DEPTH {
        return Err("Value is nested too deeply".into());
    }
    let e = |e: mlua::Error| e.to_string();
    Ok(match value {
        Value::Null => LuaValue::Nil,
        Value::Bool(b) => LuaValue::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => LuaValue::Integer(i),
            None => LuaValue::Number(n.as_f64().unwrap_or(0.)),
        },
        Value::String(s) => LuaValue::String(lua.create_string(s).map_err(e)?),
        Value::Array(items) => {
            let table = lua.create_table_with_capacity(items.len(), 0).map_err(e)?;
            for (i, item) in items.iter().enumerate() {
                table
                    .raw_set(i + 1, to_lua(lua, item, depth + 1)?)
                    .map_err(e)?;
            }
            table.set_readonly(true);
            LuaValue::Table(table)
        }
        Value::Object(map) => {
            let table = lua.create_table_with_capacity(0, map.len()).map_err(e)?;
            for (key, item) in map {
                table
                    .raw_set(key.as_str(), to_lua(lua, item, depth + 1)?)
                    .map_err(e)?;
            }
            table.set_readonly(true);
            LuaValue::Table(table)
        }
    })
}

/// Luau to JSON. A table whose keys are exactly 1..n, including an empty
/// table, is an array; a table with only string keys is an object.
pub(crate) fn from_lua(value: &LuaValue) -> Result<Value, String> {
    let mut count = 0;
    from_lua_with(value, 0, &mut count)
}

fn from_lua_with(value: &LuaValue, depth: usize, count: &mut usize) -> Result<Value, String> {
    *count += 1;
    if depth > MAX_DEPTH || *count > MAX_VALUES {
        return Err("Component data is too large or nested too deeply".into());
    }
    Ok(match value {
        LuaValue::Nil => Value::Null,
        LuaValue::Boolean(b) => Value::Bool(*b),
        LuaValue::Integer(i) => Value::Number((*i).into()),
        LuaValue::Number(n) => {
            if n.fract() == 0. && n.abs() < 9e15 {
                Value::Number((*n as i64).into())
            } else {
                Value::Number(Number::from_f64(*n).ok_or("Component numbers must be finite")?)
            }
        }
        LuaValue::String(s) => Value::String(
            s.to_str()
                .map_err(|_| "Component text must be UTF-8")?
                .to_string(),
        ),
        LuaValue::Table(table) => {
            let len = table.raw_len();
            let mut keys = 0usize;
            let mut strings = Map::new();
            let mut integers = false;
            for pair in table.pairs::<LuaValue, LuaValue>() {
                let (key, item) = pair.map_err(|e| e.to_string())?;
                keys += 1;
                match key {
                    LuaValue::String(k) => {
                        let k = k
                            .to_str()
                            .map_err(|_| "Component keys must be UTF-8")?
                            .to_string();
                        strings.insert(k, from_lua_with(&item, depth + 1, count)?);
                    }
                    LuaValue::Integer(i) if i >= 1 && (i as usize) <= len => integers = true,
                    _ => return Err("Component tables need string keys or a list".into()),
                }
            }
            if integers && !strings.is_empty() {
                return Err("A component table cannot mix list items and named fields".into());
            }
            if strings.is_empty() {
                if keys != len {
                    return Err("Component lists cannot have gaps".into());
                }
                let mut items = Vec::with_capacity(len);
                for item in table.sequence_values::<LuaValue>() {
                    let item = item.map_err(|e| e.to_string())?;
                    items.push(from_lua_with(&item, depth + 1, count)?);
                }
                Value::Array(items)
            } else {
                Value::Object(strings)
            }
        }
        LuaValue::Function(_) => {
            return Err("Components may contain only data; pass functions to controls".into());
        }
        _ => return Err("Components may contain only data".into()),
    })
}

/// Where panel files live: the user's own, then the project's.
pub fn user_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("panels")
}
pub fn project_dir(root: &Path) -> PathBuf {
    root.join(".placeholder").join("panels")
}

/// The `.luau` files in `dir`, sorted by name, at most 32.
pub fn discover(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "luau"))
        .collect();
    files.sort();
    files.truncate(32);
    files
}

/// A representative snapshot for checking panels outside the app.
pub fn sample_app() -> Value {
    serde_json::json!({
        "project": {"path": "/work/example", "name": "example", "root": "/work/example", "trusted": true},
        "git": {"files": 3, "additions": 42, "deletions": 7},
        "sessions": [
            {"id": 1, "title": "Fix the login flow", "provider": "claude", "model": "opus",
             "busy": true, "needs_approval": false, "unread": false, "archived": false,
             "pinned": true, "updated_at": 1_700_000_000_000u64, "active": true},
            {"id": 2, "title": "Write release notes", "provider": "codex", "model": null,
             "busy": false, "needs_approval": true, "unread": true, "archived": false,
             "pinned": false, "updated_at": 1_700_000_100_000u64, "active": false}
        ],
        "board": [
            {"id": "c1", "number": 1, "title": "Plan the migration", "lane": "backlog", "lead": null},
            {"id": "c2", "number": 2, "title": "Port the settings page", "lane": "in_progress", "lead": 1}
        ],
        "active_session": 1
    })
}

/// `panel check FILE [--app SNAPSHOT.json]`, `panel hook FILE` and
/// `panel paths`, for writing panels and hooks outside the app.
pub fn cli(args: &[String], data_dir: &Path) -> Result<Value, String> {
    let usage = "Usage: panel check FILE [--app SNAPSHOT.json] | panel hook FILE | panel paths";
    let read =
        |path: &str| std::fs::read_to_string(path).map_err(|e| format!("Cannot read {path}: {e}"));
    let name = |path: &str| {
        Path::new(path)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string())
    };
    match args.first().map(String::as_str) {
        Some("paths") => Ok(serde_json::json!({
            "ok": true,
            "panels": user_dir(data_dir),
            "hooks": hooks_dir(data_dir),
            "project_panels": ".placeholder/panels",
        })),
        Some("check") => {
            let file = args.get(1).ok_or(usage)?;
            let app = match args.iter().position(|a| a == "--app") {
                Some(i) => serde_json::from_str(&read(args.get(i + 1).ok_or(usage)?)?)
                    .map_err(|e| format!("Invalid snapshot: {e}"))?,
                None => sample_app(),
            };
            let source = read(file)?;
            let mut panel = PanelRuntime::new(&name(file), &source)?;
            let tree = panel.render(&app)?;
            // Press each control once, each in a fresh VM, as the visual check does.
            let mut probes = Vec::new();
            probe(&tree, &mut probes);
            for (handler, value) in probes.iter().take(16) {
                let mut fresh = PanelRuntime::new(&name(file), &source)?;
                fresh.render(&app)?;
                fresh
                    .dispatch(handler, value.clone(), &app)
                    .map_err(|e| format!("Control check failed ({handler}): {e}"))?;
            }
            let meta = panel.meta();
            Ok(serde_json::json!({
                "ok": true,
                "title": meta.title,
                "icon": meta.icon,
                "refresh": meta.refresh.map(|d| d.as_secs_f64()),
                "placement": meta.placement.as_str(),
                "slots": tree.slots().iter().map(|s| s.id()).collect::<Vec<_>>(),
                "controls": probes.len(),
            }))
        }
        Some("hook") => {
            let file = args.get(1).ok_or(usage)?;
            let hook = HookRuntime::new(&name(file), &read(file)?)?;
            let events: Vec<_> = HOOK_EVENTS.iter().filter(|e| hook.handles(e)).collect();
            Ok(serde_json::json!({"ok": true, "events": events}))
        }
        _ => Err(usage.into()),
    }
}

fn probe(node: &Node, out: &mut Vec<(String, Value)>) {
    use serde_json::json;
    match node {
        Node::Column { children, .. }
        | Node::Row { children, .. }
        | Node::Grid { children, .. }
        | Node::Card { children, .. } => children.iter().for_each(|c| probe(c, out)),
        Node::Button {
            handler,
            disabled: false,
            ..
        } => out.push((handler.clone(), Value::Null)),
        Node::Toggle { handler, value, .. } => out.push((handler.clone(), json!(!value))),
        Node::Input { handler, value, .. } => out.push((handler.clone(), json!(value))),
        Node::Select {
            handler,
            options,
            selected,
            ..
        } => out.push((handler.clone(), json!((selected + 1) % options.len()))),
        Node::Tabs {
            handler,
            labels,
            selected,
            ..
        } => out.push((handler.clone(), json!((selected + 1) % labels.len()))),
        Node::Slider { handler, min, .. } => out.push((handler.clone(), json!(min))),
        _ => {}
    }
}
