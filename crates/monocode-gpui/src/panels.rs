//! Luau panels in the right panel, and Luau hooks.
//!
//! User panels live in `<data>/panels/*.luau` and are always trusted. A
//! project may ship panels in `.placeholder/panels/*.luau`; they render, but
//! may not start agent work until the user trusts the project. Hooks live in
//! `<data>/hooks/*.luau`. Every file reloads when it changes on disk.
use crate::runtime::{Database, locked};
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Task};
use monocode_core::{
    files::path_key,
    panel::{self, ApprovalRequest, HookDecision, HookRuntime, PanelAction, PanelRuntime},
    visual::VisualDocument,
};
use notify::{RecursiveMode, Watcher};
use serde_json::Value;
use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    time::Duration,
};

const TRUST_KEY: &str = "luau.trusted_projects";
/// Icons a panel may name; anything else falls back to the dashboard icon.
const ICONS: &[&str] = &[
    "dashboard",
    "chart-square",
    "gauge",
    "bot",
    "bug",
    "rocket",
    "package",
    "hammer",
    "test-tube",
    "terminal",
    "file",
    "file-script",
    "file-diff",
    "folder-01",
    "folder-tree",
    "git-branch",
    "git-commit",
    "git-merge",
    "git-pull-request",
    "git-compare",
    "clock",
    "list-bullet",
    "queue-list",
    "message-square",
    "message-multiple",
    "chatting",
    "sparkles",
    "star",
    "pin",
    "eye",
    "globe",
    "internet",
    "palette",
    "keyboard",
    "shield-alert",
    "sliders",
    "sticky-note",
    "check-circle",
    "alert-circle",
    "inbox-hi",
    "wand",
    "ghost",
    "smile",
    "account-user",
    "cloud-upload",
    "refresh-cw",
    "play",
    "app-window",
];
pub(crate) const DEFAULT_ICON: &str = "dashboard";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct PanelId(pub u16);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Scope {
    User,
    /// A project's own panel, by the project's path key.
    Project(String),
}

#[derive(Clone, Debug)]
pub(crate) struct PanelDef {
    pub id: PanelId,
    pub path: PathBuf,
    pub scope: Scope,
    pub title: String,
    pub icon: &'static str,
    pub source: String,
    /// A load error; the panel still gets a tab that shows it.
    pub error: Option<String>,
    /// Bumped whenever the file's content changes.
    pub revision: u64,
    pub placement: monocode_core::panel::Placement,
}
impl PanelDef {
    pub fn file_name(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
    /// The file name without `.luau`, which `host.select_tab` also accepts.
    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
    pub fn document(&self) -> VisualDocument {
        VisualDocument {
            id: format!("panel-{}", self.id.0),
            title: self.file_name(),
            source: self.source.clone(),
        }
    }
}

struct HookSlot {
    path: PathBuf,
    source: String,
    runtime: Option<HookRuntime>,
    error: Option<String>,
}

/// The panels changed: added, removed, renamed or edited.
pub(crate) struct RegistryChanged;
/// An app event panels may handle with `on`.
#[derive(Clone, Debug)]
pub(crate) struct PanelEvent {
    pub name: &'static str,
    pub payload: Value,
}
/// Actions that hooks queued, for the shell to run.
pub(crate) struct HookActions(pub Vec<PanelAction>);
/// A panel or hook failed to load, or a hook failed while running.
pub(crate) struct Problem {
    pub source: String,
    pub message: String,
}

pub(crate) struct Registry {
    store: Database,
    user_dir: Option<PathBuf>,
    hooks_dir: Option<PathBuf>,
    defs: Vec<PanelDef>,
    /// Project roots by path key, for discovery.
    projects: HashMap<String, PathBuf>,
    hooks: Vec<HookSlot>,
    trusted: BTreeSet<String>,
    next_id: u16,
    _watcher: Option<notify::RecommendedWatcher>,
    watched: BTreeSet<PathBuf>,
    _reload: Option<Task<()>>,
    changes: Option<async_channel::Sender<()>>,
}
impl EventEmitter<RegistryChanged> for Registry {}
impl EventEmitter<PanelEvent> for Registry {}
impl EventEmitter<HookActions> for Registry {}
impl EventEmitter<Problem> for Registry {}

pub(crate) struct Shared(pub Entity<Registry>);
impl Global for Shared {}

pub(crate) fn get(cx: &App) -> Option<Entity<Registry>> {
    cx.try_global::<Shared>().map(|s| s.0.clone())
}

pub(crate) fn install(store: Database, cx: &mut App) -> Entity<Registry> {
    let data_dir = locked(&store)
        .ok()
        .and_then(|s| s.data_dir().map(Path::to_path_buf));
    install_at(store, data_dir, cx)
}

/// Like `install`, with panels and hooks read from `data_dir`.
pub(crate) fn install_at(
    store: Database,
    data_dir: Option<PathBuf>,
    cx: &mut App,
) -> Entity<Registry> {
    if let Some(registry) = get(cx) {
        return registry;
    }
    let registry = cx.new(|cx| Registry::new(store, data_dir, cx));
    cx.set_global(Shared(registry.clone()));
    registry
}

impl Registry {
    fn new(store: Database, data_dir: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let user_dir = data_dir.as_deref().map(panel::user_dir);
        let hooks_dir = data_dir.as_deref().map(panel::hooks_dir);
        for dir in user_dir.iter().chain(&hooks_dir) {
            let _ = std::fs::create_dir_all(dir);
        }
        let trusted = locked(&store)
            .ok()
            .and_then(|s| s.setting(TRUST_KEY).ok().flatten())
            .and_then(|v| serde_json::from_str::<BTreeSet<String>>(&v).ok())
            .unwrap_or_default();
        let (tx, rx) = async_channel::bounded::<()>(1);
        let reload = cx.spawn(async move |this, cx| {
            while rx.recv().await.is_ok() {
                // Editors write in several steps; settle before reading.
                cx.background_executor()
                    .timer(Duration::from_millis(150))
                    .await;
                while rx.try_recv().is_ok() {}
                if this.update(cx, |this, cx| this.reload(cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            store,
            user_dir,
            hooks_dir,
            defs: Vec::new(),
            projects: HashMap::new(),
            hooks: Vec::new(),
            trusted,
            next_id: 0,
            _watcher: None,
            watched: BTreeSet::new(),
            _reload: Some(reload),
            changes: Some(tx),
        };
        this.reload(cx);
        this
    }

    /// Discovers `root`'s own panels, once per project.
    pub(crate) fn add_project(&mut self, root: &str, cx: &mut Context<Self>) {
        if root.is_empty() || root.starts_with("remote://") {
            return;
        }
        let key = path_key(root);
        if self.projects.contains_key(&key) {
            return;
        }
        self.projects.insert(key, PathBuf::from(root));
        self.reload(cx);
    }

    /// The panels shown for `project`: the user's, then the project's.
    pub(crate) fn panels_for(&self, project: &str) -> Vec<&PanelDef> {
        let key = path_key(project);
        self.defs
            .iter()
            .filter(|d| match &d.scope {
                Scope::User => true,
                Scope::Project(p) => *p == key,
            })
            .collect()
    }
    pub(crate) fn panel(&self, id: PanelId) -> Option<&PanelDef> {
        self.defs.iter().find(|d| d.id == id)
    }
    pub(crate) fn trusted(&self, project: &str) -> bool {
        self.trusted.contains(&path_key(project))
    }
    pub(crate) fn set_trusted(&mut self, project: &str, trusted: bool, cx: &mut Context<Self>) {
        let key = path_key(project);
        let changed = if trusted {
            self.trusted.insert(key)
        } else {
            self.trusted.remove(&key)
        };
        if !changed {
            return;
        }
        if let Ok(json) = serde_json::to_string(&self.trusted)
            && let Ok(store) = locked(&self.store)
        {
            let _ = store.save_setting(TRUST_KEY, &json);
        }
        cx.emit(RegistryChanged);
        cx.notify();
    }
    /// Whether `def` may run `action`. A project's panels need trust before
    /// they can make agents work.
    pub(crate) fn allows(&self, def: &PanelDef, action: &PanelAction) -> bool {
        match &def.scope {
            Scope::User => true,
            Scope::Project(key) => !action.starts_work() || self.trusted.contains(key),
        }
    }

    /// Asks every hook about a permission request. Any deny wins.
    pub(crate) fn approval(
        &mut self,
        request: &ApprovalRequest,
        cx: &mut Context<Self>,
    ) -> Option<HookDecision> {
        let mut decisions = Vec::new();
        let mut actions = Vec::new();
        let mut problems = Vec::new();
        for slot in &mut self.hooks {
            let Some(runtime) = slot.runtime.as_mut() else {
                continue;
            };
            if !runtime.handles("approval") {
                continue;
            }
            match runtime.approval(request) {
                Ok(Some(decision)) => decisions.push(decision),
                Ok(None) => {}
                Err(error) => problems.push(fail(slot, error)),
            }
            if let Some(runtime) = slot.runtime.as_mut() {
                actions.extend(runtime.take_actions());
            }
        }
        self.report(problems, cx);
        if !actions.is_empty() {
            cx.emit(HookActions(actions));
        }
        panel::combine(decisions)
    }

    /// Tells hooks and panels about an app event.
    pub(crate) fn emit_event(
        &mut self,
        name: &'static str,
        payload: Value,
        cx: &mut Context<Self>,
    ) {
        let mut actions = Vec::new();
        let mut problems = Vec::new();
        for slot in &mut self.hooks {
            let Some(runtime) = slot.runtime.as_mut() else {
                continue;
            };
            if !runtime.handles(name) {
                continue;
            }
            let result = runtime.call(name, &payload);
            actions.extend(runtime.take_actions());
            if let Err(error) = result {
                problems.push(fail(slot, error));
            }
        }
        self.report(problems, cx);
        if !actions.is_empty() {
            cx.emit(HookActions(actions));
        }
        cx.emit(PanelEvent { name, payload });
    }

    fn report(&self, problems: Vec<Problem>, cx: &mut Context<Self>) {
        for problem in problems {
            log::warn!("{}: {}", problem.source, problem.message);
            cx.emit(problem);
        }
    }

    /// Tells only panels about an event, as for a request hooks left open.
    pub(crate) fn notify_panels(
        &mut self,
        name: &'static str,
        payload: Value,
        cx: &mut Context<Self>,
    ) {
        cx.emit(PanelEvent { name, payload });
    }

    /// Reads every panel and hook file again, keeping ids for unchanged paths.
    fn reload(&mut self, cx: &mut Context<Self>) {
        let mut found: Vec<(PathBuf, Scope)> = Vec::new();
        if let Some(dir) = &self.user_dir {
            found.extend(panel::discover(dir).into_iter().map(|p| (p, Scope::User)));
        }
        let mut projects: Vec<_> = self.projects.iter().collect();
        projects.sort();
        for (key, root) in projects {
            found.extend(
                panel::discover(&panel::project_dir(root))
                    .into_iter()
                    .map(|p| (p, Scope::Project(key.clone()))),
            );
        }
        let mut changed = found.len() != self.defs.len();
        let mut defs = Vec::with_capacity(found.len());
        for (path, scope) in found {
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            let previous = self
                .defs
                .iter()
                .find(|d| d.path == path && d.scope == scope);
            if let Some(previous) = previous
                && previous.source == source
            {
                defs.push(previous.clone());
                continue;
            }
            changed = true;
            let id = previous.map(|d| d.id).unwrap_or_else(|| {
                self.next_id = self.next_id.wrapping_add(1);
                PanelId(self.next_id)
            });
            let revision = previous.map_or(0, |d| d.revision + 1);
            let stem = path
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let (title, icon, placement, error) = match PanelRuntime::new(&name, &source) {
                Ok(runtime) => {
                    let meta = runtime.meta();
                    (meta.title.clone(), meta.icon.clone(), meta.placement, None)
                }
                Err(error) => (None, None, Default::default(), Some(error)),
            };
            defs.push(PanelDef {
                id,
                path,
                scope,
                title: title.unwrap_or_else(|| title_case(&stem)),
                icon: icon
                    .and_then(|i| ICONS.iter().find(|known| **known == i).copied())
                    .unwrap_or(DEFAULT_ICON),
                source,
                error,
                revision,
                placement,
            });
        }
        let mut problems: Vec<Problem> = defs
            .iter()
            .filter(|d| {
                d.error.is_some()
                    && !self
                        .defs
                        .iter()
                        .any(|old| old.path == d.path && old.source == d.source)
            })
            .map(|d| Problem {
                source: d.file_name(),
                message: d.error.clone().unwrap_or_default(),
            })
            .collect();
        self.defs = defs;
        if self.reload_hooks() {
            changed = true;
            problems.extend(self.hooks.iter().filter_map(|h| {
                h.error.as_ref().map(|e| Problem {
                    source: h
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    message: e.clone(),
                })
            }));
        }
        self.report(problems, cx);
        self.rewatch();
        if changed {
            cx.emit(RegistryChanged);
            cx.notify();
        }
    }

    fn reload_hooks(&mut self) -> bool {
        let files: Vec<(PathBuf, String)> = self
            .hooks_dir
            .as_deref()
            .map(panel::discover)
            .unwrap_or_default()
            .into_iter()
            .map(|path| {
                let source = std::fs::read_to_string(&path).unwrap_or_default();
                (path, source)
            })
            .collect();
        let unchanged = files.len() == self.hooks.len()
            && files
                .iter()
                .zip(&self.hooks)
                .all(|((path, source), slot)| slot.path == *path && slot.source == *source);
        if unchanged {
            return false;
        }
        self.hooks = files
            .into_iter()
            .map(|(path, source)| {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let (runtime, error) = match HookRuntime::new(&name, &source) {
                    Ok(runtime) => (Some(runtime), None),
                    Err(error) => {
                        log::warn!("{error}");
                        (None, Some(error))
                    }
                };
                HookSlot {
                    path,
                    source,
                    runtime,
                    error,
                }
            })
            .collect();
        true
    }

    /// Watches the panel and hook folders, and each project's `.placeholder`
    /// chain so a newly created panels folder is noticed.
    fn rewatch(&mut self) {
        // Watcher threads wake the foreground executor, which tests forbid.
        if cfg!(test) {
            return;
        }
        let Some(tx) = self.changes.clone() else {
            return;
        };
        let mut wanted = BTreeSet::new();
        for dir in self.user_dir.iter().chain(&self.hooks_dir) {
            wanted.insert(dir.clone());
        }
        for root in self.projects.values() {
            let dot = root.join(".placeholder");
            let panels = panel::project_dir(root);
            for dir in [root.clone(), dot, panels] {
                if dir.is_dir() {
                    wanted.insert(dir);
                }
            }
        }
        if wanted == self.watched && self._watcher.is_some() {
            return;
        }
        let watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let relevant = match &event {
                Ok(event) => {
                    !event.kind.is_access()
                        && event.paths.iter().any(|p| {
                            p.extension().is_some_and(|x| x == "luau")
                                || p.file_name()
                                    .is_some_and(|n| n == ".placeholder" || n == "panels")
                        })
                }
                Err(_) => true,
            };
            if relevant {
                let _ = tx.try_send(());
            }
        });
        let Ok(mut watcher) = watcher else {
            return;
        };
        for dir in &wanted {
            let _ = watcher.watch(dir, RecursiveMode::NonRecursive);
        }
        self._watcher = Some(watcher);
        self.watched = wanted;
    }
}

/// Records a hook failure on its slot and describes it.
fn fail(slot: &mut HookSlot, error: String) -> Problem {
    slot.error = Some(error.clone());
    Problem {
        source: slot
            .path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "hook".into()),
        message: error,
    }
}

/// `git-stats` → `Git stats`.
fn title_case(stem: &str) -> String {
    let words = stem.replace(['-', '_'], " ");
    let mut chars = words.trim().chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Panel".into(),
    }
}
