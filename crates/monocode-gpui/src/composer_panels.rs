//! Luau panels with `placement = "composer"`: drawn below the composer of a
//! new, empty session instead of as right-panel tabs. One shared host keeps
//! their views, so every empty session shows the same panel state.
use crate::{
    panels::{PanelEvent, PanelId, RegistryChanged},
    visuals::{PanelActions, VisualChanged, VisualView},
};
use gpui::{App, AppContext, Context, Entity, EventEmitter, Global, Subscription};
use monocode_core::panel::{PanelAction, Placement};
use serde_json::Value;

/// A composer panel asked for app actions; the shell checks and runs them.
pub(crate) struct ComposerPanelActions(pub PanelId, pub Vec<PanelAction>);

struct Slot {
    id: PanelId,
    revision: u64,
    view: Entity<VisualView>,
    _subscriptions: [Subscription; 2],
}

pub(crate) struct ComposerPanels {
    project: String,
    app: Option<Value>,
    /// The current project's composer panels, whether or not they have views yet.
    wanted: Vec<PanelId>,
    slots: Vec<Slot>,
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
                this.sync(cx)
            }));
            subscriptions.push(cx.subscribe(&registry, |this, _, event: &PanelEvent, cx| {
                for slot in &this.slots {
                    slot.view
                        .update(cx, |v, cx| v.deliver(event.name, &event.payload, cx));
                }
            }));
        }
        Self {
            project: String::new(),
            app: None,
            wanted: Vec::new(),
            slots: Vec::new(),
            _subscriptions: subscriptions,
        }
    }

    pub(crate) fn set_project(&mut self, project: &str, cx: &mut Context<Self>) {
        if self.project == project {
            return;
        }
        self.project = project.to_string();
        if let Some(registry) = crate::panels::get(cx) {
            registry.update(cx, |r, cx| r.add_project(project, cx));
        }
        self.sync(cx);
    }

    /// Whether the project has composer panels, so a snapshot is worth building.
    pub(crate) fn wanted(&self) -> bool {
        !self.wanted.is_empty()
    }

    pub(crate) fn set_app(&mut self, app: &Value, cx: &mut Context<Self>) {
        if self.app.as_ref() == Some(app) {
            return;
        }
        for slot in &self.slots {
            slot.view.update(cx, |v, cx| v.set_app(app, cx));
        }
        self.app = Some(app.clone());
        self.sync(cx);
    }

    pub(crate) fn views(&self) -> Vec<Entity<VisualView>> {
        self.slots.iter().map(|s| s.view.clone()).collect()
    }

    /// Matches the views to the registry, reloading edited files in place.
    fn sync(&mut self, cx: &mut Context<Self>) {
        let Some(registry) = crate::panels::get(cx) else {
            return;
        };
        let defs: Vec<_> = registry
            .read(cx)
            .panels_for(&self.project)
            .into_iter()
            .filter(|d| d.placement == Placement::Composer)
            .cloned()
            .collect();
        self.wanted = defs.iter().map(|d| d.id).collect();
        let mut old = std::mem::take(&mut self.slots);
        for def in defs {
            if let Some(ix) = old.iter().position(|s| s.id == def.id) {
                let mut slot = old.remove(ix);
                if slot.revision != def.revision {
                    slot.revision = def.revision;
                    slot.view.update(cx, |v, cx| v.reload(def.document(), cx));
                }
                self.slots.push(slot);
                continue;
            }
            // A view needs its first snapshot before it can render.
            let Some(app) = self.app.clone() else {
                continue;
            };
            let id = def.id;
            let document = def.document();
            let view = cx.new(|cx| VisualView::new_panel(document, app, Default::default(), cx));
            let subscriptions = [
                cx.subscribe(&view, |_, _, _: &VisualChanged, cx| cx.notify()),
                cx.subscribe(&view, move |_, _, event: &PanelActions, cx| {
                    cx.emit(ComposerPanelActions(id, event.0.clone()));
                }),
            ];
            start_refresh(&view, cx);
            self.slots.push(Slot {
                id,
                revision: def.revision,
                view,
                _subscriptions: subscriptions,
            });
        }
        cx.notify();
    }
}

/// Renders a panel again on its declared interval while its view exists.
fn start_refresh(view: &Entity<VisualView>, cx: &mut Context<ComposerPanels>) {
    let weak = view.downgrade();
    cx.spawn(async move |_, cx| {
        loop {
            let Ok(interval) = weak.read_with(cx, |v, _| v.refresh_interval()) else {
                break;
            };
            cx.background_executor()
                .timer(interval.unwrap_or(std::time::Duration::from_secs(1)))
                .await;
            if interval.is_some() && weak.update(cx, |v, cx| v.rerender(cx)).is_err() {
                break;
            }
        }
    })
    .detach();
}
