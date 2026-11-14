//! User hooks written in Luau. A hook file returns a table of functions named
//! after app events. `approval` may answer a provider's permission request;
//! every other hook only observes, and may queue `host` actions.
use super::{PanelAction, SDK, Sandbox, from_lua, take_actions, to_lua};
use mlua::{Function, Table, Value as LuaValue};
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Events hooks can observe. `approval` is the only one with an answer.
pub const HOOK_EVENTS: &[&str] = &[
    "approval",
    "turn_started",
    "turn_finished",
    "session_opened",
];

/// What an `approval` hook asks for. Hooks that return nothing leave the
/// request to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookDecision {
    Allow,
    /// Allow this and matching requests for the rest of the session.
    Session,
    Deny,
}

/// The permission request as a hook sees it.
#[derive(Clone, Debug, Serialize)]
pub struct ApprovalRequest {
    pub session: i64,
    pub provider: String,
    pub title: String,
    pub detail: String,
    pub cwd: String,
    pub project: String,
    /// Whether the provider supports "allow for this session".
    pub can_remember: bool,
}

pub struct HookRuntime {
    name: String,
    path: PathBuf,
    sandbox: Sandbox,
    controller: Table,
    events: Vec<String>,
}

impl HookRuntime {
    pub fn new(name: &str, source: &str) -> Result<Self, String> {
        let (sandbox, controller) = Sandbox::new(SDK, name)?;
        let program = sandbox.load(name, source, &controller)?;
        let init: Function = controller.get("initHook").map_err(|e| e.to_string())?;
        let names: Vec<String> = sandbox.guard(name, || init.call(program))?;
        if let Some(unknown) = names.iter().find(|n| !HOOK_EVENTS.contains(&n.as_str())) {
            return Err(format!(
                "{name}: unknown hook `{unknown}`; hooks are {}",
                HOOK_EVENTS.join(", ")
            ));
        }
        Ok(Self {
            name: name.to_string(),
            path: PathBuf::new(),
            sandbox,
            controller,
            events: names,
        })
    }
    pub fn load(path: &Path) -> Result<Self, String> {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "hook".into());
        let source = std::fs::read_to_string(path).map_err(|e| format!("{name}: {e}"))?;
        let mut hook = Self::new(&name, &source)?;
        hook.path = path.to_path_buf();
        Ok(hook)
    }
    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn handles(&self, event: &str) -> bool {
        self.events.iter().any(|e| e == event)
    }

    /// Calls one hook and returns its JSON result.
    pub fn call(&mut self, event: &str, payload: &Value) -> Result<Value, String> {
        if !self.handles(event) {
            return Ok(Value::Null);
        }
        let payload = to_lua(&self.sandbox.lua, payload, 0)?;
        let hook: Function = self.controller.get("hook").map_err(|e| e.to_string())?;
        let result: LuaValue = self
            .sandbox
            .guard(&self.name, || hook.call((event, payload)))?;
        from_lua(&result).map_err(|e| format!("{}: {e}", self.name))
    }

    pub fn approval(&mut self, request: &ApprovalRequest) -> Result<Option<HookDecision>, String> {
        let payload = serde_json::to_value(request).map_err(|e| e.to_string())?;
        match self.call("approval", &payload)? {
            Value::Null => Ok(None),
            Value::String(s) => match s.as_str() {
                "allow" => Ok(Some(HookDecision::Allow)),
                "session" if request.can_remember => Ok(Some(HookDecision::Session)),
                "session" => Ok(Some(HookDecision::Allow)),
                "deny" => Ok(Some(HookDecision::Deny)),
                "ask" => Ok(None),
                other => Err(format!(
                    "{}: approval must return \"allow\", \"session\", \"deny\", \"ask\" or nil, not \"{other}\"",
                    self.name
                )),
            },
            _ => Err(format!(
                "{}: approval must return a string or nil",
                self.name
            )),
        }
    }

    pub fn take_actions(&mut self) -> Vec<PanelAction> {
        take_actions(&self.sandbox, &self.controller, &self.name)
    }
}

/// Where hook files live.
pub fn hooks_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("hooks")
}

/// Combines hook answers: any deny wins, then allow; otherwise ask the user.
pub fn combine(decisions: impl IntoIterator<Item = HookDecision>) -> Option<HookDecision> {
    let mut out = None;
    for decision in decisions {
        out = Some(match (out, decision) {
            (_, HookDecision::Deny) | (Some(HookDecision::Deny), _) => HookDecision::Deny,
            (Some(HookDecision::Allow), _) | (_, HookDecision::Allow) => HookDecision::Allow,
            _ => HookDecision::Session,
        });
    }
    out
}
