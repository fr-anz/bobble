//! Luau panels with `placement = "composer"`: drawn below the composer of a
//! new, empty session instead of as right-panel tabs. One shared host keeps
//! their views per project, so a project's empty sessions show the same
//! panel state.
use crate::{
    panels::{PanelEvent, PanelId, RegistryChanged},
    visuals::{PanelActions, VisualChanged, VisualView},
};
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription, Task};
use monocode_core::{
    files::path_key,
    panel::{PanelAction, Placement},
};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc};

/// A composer panel asked for app actions; the shell showing its project
/// (by path key) checks and runs them.
pub(crate) struct ComposerPanelActions(pub PanelId, pub Vec<PanelAction>, pub String);

struct Slot {
    id: PanelId,
    revision: u64,
    view: Entity<VisualView>,
    /// Missed a timed render while off screen; render when shown again.
    stale: bool,
    /// The panel's timed render, when it declares `refresh`.
    _refresh: Option<Task<()>>,
    _subscriptions: [Subscription; 2],
}

/// One project's composer panels. Each project keeps its own, so windows on
/// different projects never take turns rebuilding a shared set.
#[derive(Default)]
struct Group {
    app: Option<Arc<Value>>,
    /// Whether an empty session is on screen to draw the panels.
    shown: bool,
    /// The project's composer panels, whether or not they have views yet.
    wanted: Vec<PanelId>,
    slots: Vec<Slot>,
}

pub(crate) struct ComposerPanels {
    /// By project path key.
    groups: HashMap<String, Group>,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<ComposerPanelActions> for ComposerPanels {}

struct Shared(Entity<ComposerPanels>);
impl Global for Shared {}

pub(crate) fn get(cx: &App) -> Option<Entity<ComposerPanels>> {
    cx.try_global::<Shared>().map(|s| s.0.clone())
}

/// Installs the host once; needs the panel registry installed first.
pub(crate) fn install(cx: &mut App) -> Entity<ComposerPanels> {
    if let Some(host) = get(cx) {
        return host;
    }
    let host = cx.new(ComposerPanels::new);
    cx.set_global(Shared(host.clone()));
    host
}

impl ComposerPanels {
    fn new(cx: &mut Context<Self>) -> Self {
        let mut subscriptions = Vec::new();
        if let Some(registry) = crate::panels::get(cx) {
            subscriptions.push(cx.subscribe(&registry, |this, _, _: &RegistryChanged, cx| {
                let keys: Vec<String> = this.groups.keys().cloned().collect();
                for key in keys {
                    this.sync(&key, cx);
                }
            }));
            subscriptions.push(cx.subscribe(&registry, |this, _, event: &PanelEvent, cx| {
                for slot in this.groups.values().flat_map(|g| &g.slots) {
                    slot.view
                        .update(cx, |v, cx| v.deliver(event.name, &event.payload, cx));
                }
            }));
        }
        Self {
            groups: HashMap::new(),
            _subscriptions: subscriptions,
        }
    }

    fn group(&self, project: &str) -> Option<&Group> {
        self.groups.get(&path_key(project))
    }

    pub(crate) fn has_project(&self, project: &str) -> bool {
        self.group(project).is_some()
    }
    /// Starts keeping `project`'s composer panels, once per project.
    pub(crate) fn add_project(&mut self, project: &str, cx: &mut Context<Self>) {
        let key = path_key(project);
        if self.groups.contains_key(&key) {
            return;
        }
        self.groups.insert(key.clone(), Group::default());
        if let Some(registry) = crate::panels::get(cx) {
            registry.update(cx, |r, cx| r.add_project(project, cx));
        }
        self.sync(&key, cx);
    }

    /// Whether the project has composer panels, so a snapshot is worth building.
    pub(crate) fn wanted(&self, project: &str) -> bool {
        self.group(project).is_some_and(|g| !g.wanted.is_empty())
    }
    pub(crate) fn has_app(&self, project: &str) -> bool {
        self.group(project).is_some_and(|g| g.app.is_some())
    }
    /// Whether `app` is the snapshot the project's panels already render from.
    pub(crate) fn has(&self, project: &str, app: &Arc<Value>) -> bool {
        self.group(project)
            .and_then(|g| g.app.as_ref())
            .is_some_and(|a| Arc::ptr_eq(a, app))
    }
    pub(crate) fn shown(&self, project: &str) -> bool {
        self.group(project).is_some_and(|g| g.shown)
    }
    /// Timed renders pause while no empty session is on screen; the ones
    /// missed run when the panels show again.
    pub(crate) fn set_shown(&mut self, project: &str, shown: bool, cx: &mut Context<Self>) {
        let Some(group) = self.groups.get_mut(&path_key(project)) else {
            return;
        };
        group.shown = shown;
        if !shown {
            return;
        }
        for slot in &mut group.slots {
            if std::mem::take(&mut slot.stale) {
                slot.view.update(cx, |v, cx| v.rerender(cx));
            }
        }
    }

    pub(crate) fn set_app(&mut self, project: &str, app: Arc<Value>, cx: &mut Context<Self>) {
        let key = path_key(project);
        let Some(group) = self.groups.get_mut(&key) else {
            return;
        };
        if group.app.as_ref().is_some_and(|a| Arc::ptr_eq(a, &app)) {
            return;
        }
        for slot in &group.slots {
            slot.view.update(cx, |v, cx| v.set_app(&app, cx));
        }
        group.app = Some(app);
        // Panels found before the first snapshot were waiting for it.
        if group.slots.len() != group.wanted.len() {
            self.sync(&key, cx);
        }
    }

    /// The views drawn below the composer of `project`'s empty sessions.
    pub(crate) fn views(&self, project: &str) -> Vec<Entity<VisualView>> {
        self.group(project)
            .map(|g| g.slots.iter().map(|s| s.view.clone()).collect())
            .unwrap_or_default()
    }

    /// Matches a project's views to the registry, reloading edited files in
    /// place. Redraws the empty sessions only when the set of views changed;
    /// a view that changed redraws them itself.
    fn sync(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(registry) = crate::panels::get(cx) else {
            return;
        };
        let defs: Vec<_> = registry
            .read(cx)
            .panels_for(key)
            .into_iter()
            .filter(|d| d.placement == Placement::Composer)
            .cloned()
            .collect();
        let Some(mut group) = self.groups.remove(key) else {
            return;
        };
        group.wanted = defs.iter().map(|d| d.id).collect();
        let before: Vec<PanelId> = group.slots.iter().map(|s| s.id).collect();
        let mut old = std::mem::take(&mut group.slots);
        for def in defs {
            if let Some(ix) = old.iter().position(|s| s.id == def.id) {
                let mut slot = old.remove(ix);
                if slot.revision != def.revision {
                    slot.revision = def.revision;
                    slot.view.update(cx, |v, cx| v.reload(def.document(), cx));
                    // The edit may have added, changed or dropped `refresh`.
                    slot._refresh = refresh(key, slot.id, &slot.view, cx);
                }
                group.slots.push(slot);
                continue;
            }
            // A view needs its first snapshot before it can render.
            let Some(app) = group.app.clone() else {
                continue;
            };
            let id = def.id;
            let document = def.document();
            let view = cx.new(|cx| VisualView::new_panel(document, app, Default::default(), cx));
            let project = key.to_string();
            let subscriptions = [
                cx.subscribe(&view, |_, _, _: &VisualChanged, cx| cx.notify()),
                cx.subscribe(&view, move |_, _, event: &PanelActions, cx| {
                    cx.emit(ComposerPanelActions(id, event.0.clone(), project.clone()));
                }),
            ];
            group.slots.push(Slot {
                id,
                revision: def.revision,
                stale: false,
                _refresh: refresh(key, id, &view, cx),
                view,
                _subscriptions: subscriptions,
            });
        }
        let changed = before != group.slots.iter().map(|s| s.id).collect::<Vec<_>>();
        self.groups.insert(key.to_string(), group);
        if changed {
            cx.notify();
        }
    }
}

/// Renders a panel again on its declared interval, while it is on screen.
/// A panel without `refresh` gets no timer at all.
fn refresh(
    key: &str,
    id: PanelId,
    view: &Entity<VisualView>,
    cx: &mut Context<ComposerPanels>,
) -> Option<Task<()>> {
    let interval = view.read(cx).refresh_interval()?;
    let weak = view.downgrade();
    let key = key.to_string();
    Some(cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(interval).await;
            let Ok(shown) = this.update(cx, |this, _| {
                let Some(group) = this.groups.get_mut(&key) else {
                    return false;
                };
                if !group.shown
                    && let Some(slot) = group.slots.iter_mut().find(|s| s.id == id)
                {
                    slot.stale = true;
                }
                group.shown
            }) else {
                break;
            };
            if shown && weak.update(cx, |v, cx| v.rerender(cx)).is_err() {
                break;
            }
        }
    }))
}
