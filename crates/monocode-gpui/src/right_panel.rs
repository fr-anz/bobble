//! Sidebar.tsx's inset `RightPanel`: the project tools beside the card, with
//! reorderable Explorer / Changes / History tabs in the title bar, and a
//! trailing Terminal tab when the terminal lives in the panel.
use crate::{
    changes::Changes,
    explorer::Explorer,
    git_status::{GitChanged, ProjectGit},
    history::History,
    panels::{PanelId, RegistryChanged},
    project_tabs::{TabLabel, TabReveal, tab_button},
    theme::*,
    visuals::{PanelActions, VisualChanged, VisualView},
};
use monocode_core::{panel::PanelAction, visual::SlotName};
use gpui::{
    AnyElement, App, Bounds, Context, Entity, EventEmitter, FocusHandle, Focusable, FontFeatures,
    FontWeight, Hsla, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Role, SharedString,
    Stateful, StyleRefinement, Subscription, WeakEntity, Window, div, prelude::*, px,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub(crate) const WIDTH_MIN: f32 = 220.;
pub(crate) const WIDTH_MAX: f32 = 640.;
pub(crate) const WIDTH_DEFAULT: f32 = 288.;
/// `count` 26px icon tabs, one unfolded label with 4px extra padding, and the gaps.
pub(crate) fn tabs_width(count: usize) -> f32 {
    tab_icons_width(count) + 64. + 4.
}
pub(crate) fn tab_icons_width(count: usize) -> f32 {
    count as f32 * 26. + count.saturating_sub(1) as f32 * 2.
}

pub(crate) fn tab_label_width(width: f32, count: usize) -> f32 {
    // The selected tab adds 2px of padding per side. Below 30px there is
    // insufficient room for a useful abbreviated label and its 6px gap.
    let available = (width - tab_icons_width(count) - 4.).max(0.);
    if available < 30. { 0. } else { available }
}

// Separators inside the inset card use its 4% hairline; dark selection is 10%.
pub(crate) fn stroke() -> Hsla {
    content(0.04)
}
pub(crate) fn selection() -> Hsla {
    content(0.1)
}
pub(crate) const DIFF_ADD: u32 = 0x6ee7b7;
pub(crate) const DIFF_DEL: u32 = 0xfda4af;
// Tailwind v4.
pub(crate) const AMBER_400: u32 = 0xffb900;
pub(crate) const SKY_400: u32 = 0x00bcff;
pub(crate) const RED_400: u32 = 0xff6467;
/// `--color-background-base` in the default dark theme, hsl(240 0% 9%).
pub(crate) const BACKGROUND_BASE: u32 = 0x171717;

pub(crate) fn tabular() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub(crate) enum RightTab {
    #[default]
    Files,
    Changes,
    Graph,
    /// The project's terminal, when it opens in the panel. Always last and
    /// never part of the saved order.
    Terminal,
    /// A Luau panel. Panels follow the built-in tabs and are never saved.
    #[serde(skip)]
    Script(PanelId),
}
impl RightTab {
    fn label(self) -> &'static str {
        match self {
            Self::Files => "Explorer",
            Self::Changes => "Changes",
            Self::Graph => "History",
            Self::Terminal => "Terminal",
            Self::Script(_) => "Panel",
        }
    }
    /// Whether the tab can be saved and restored.
    pub(crate) fn saved(self) -> bool {
        !matches!(self, Self::Script(_))
    }
    /// loadRightPanelTabOrder: unique known tabs, the missing ones appended.
    /// Changes has no tab; the title bar's diff stat opens it.
    pub(crate) fn normalize(order: &[RightTab]) -> Vec<RightTab> {
        let mut out = Vec::new();
        for tab in order.iter().chain(&[Self::Files, Self::Graph]) {
            if !matches!(tab, Self::Terminal | Self::Changes | Self::Script(_))
                && !out.contains(tab)
            {
                out.push(*tab);
            }
        }
        out
    }
}

pub(crate) enum PanelEvent {
    Tab(RightTab),
    Reordered(Vec<RightTab>),
    /// A Luau panel asked for app actions. Untrusted actions are removed.
    Actions(PanelId, Vec<PanelAction>),
}

/// A Luau panel's tab and, once opened, its view.
struct ScriptTab {
    id: PanelId,
    title: &'static str,
    icon: &'static str,
    element_id: &'static str,
    revision: u64,
    view: Option<(Entity<VisualView>, [Subscription; 2])>,
}

/// Interns a short dynamic label for APIs that keep `&'static str`. Panel
/// names are few and bounded, so the leak is too.
pub(crate) fn intern(text: &str) -> &'static str {
    use std::{collections::HashSet, sync::Mutex};
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let names = names.get_or_insert_with(HashSet::new);
    if let Some(found) = names.get(text) {
        return found;
    }
    let leaked: &'static str = Box::leak(text.to_owned().into_boxed_str());
    names.insert(leaked);
    leaked
}

/// Tool headers share the open-file tab bar's height and bottom rule.
pub(crate) fn pane_header(id: &'static str, label: &'static str) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .debug_selector(move || id.into())
        .role(Role::Toolbar)
        .aria_label(label)
        .h(px(PANE_HEADER_HEIGHT))
        .flex_shrink_0()
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .px_3()
        .border_b_1()
        .border_color(stroke())
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(content(0.85))
}

/// GitChangesPanel's `IconAction`: a 20px square, ink at 55%.
pub(crate) fn icon_action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<SharedString>,
    path: &'static str,
    enabled: bool,
) -> Stateful<gpui::Div> {
    let id = id.into();
    let group: SharedString = format!("icon-action-{id:?}").into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.into())
        .group(group.clone())
        .size(px(20.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .when(!enabled, |d| d.opacity(0.4))
        .when(enabled, |d| d.hover(|s| s.bg(content(0.1))))
        .child(
            icon(path, 14.)
                .text_color(content(0.55))
                .when(enabled, |i| {
                    i.group_hover(group, |s| s.text_color(content(1.)))
                }),
        )
}

/// The `animate-spin` Loader.
pub(crate) fn spinner(id: impl Into<gpui::ElementId>, path: &'static str, side: f32) -> AnyElement {
    use gpui::{Transformation, percentage};
    crate::pulse::looping(
        id,
        std::time::Duration::from_secs(1),
        crate::pulse::Rate::Smooth,
        move |delta| {
            icon(path, side).with_transformation(Transformation::rotate(percentage(delta)))
        },
    )
    .into_any_element()
}

pub(crate) fn format_integer(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    if value < 0 { format!("-{out}") } else { out }
}

pub(crate) fn diff_stat_width(stats: Option<&monocode_core::scm::DiffStats>) -> f32 {
    let added = format_integer(stats.map_or(0, |s| s.additions.max(0)));
    let deleted = format_integer(stats.map_or(0, |s| s.deletions.max(0)));
    // Tabular digits at 10px, the signs, a 6px gap and 6px padding per side.
    (added.len() + deleted.len() + 2) as f32 * 6.5 + 18.
}

/// Fit the complete diff label together so one number cannot push the other
/// outside its slot. Callers include any gap that scales with their text.
pub(crate) fn diff_stat_scale(available: f32, natural: f32) -> f32 {
    (available.max(0.) / natural.max(f32::EPSILON)).clamp(0.1, 1.)
}

pub(crate) fn diff_stat(
    stats: Option<&monocode_core::scm::DiffStats>,
    width: f32,
) -> Stateful<gpui::Div> {
    let added = format_integer(stats.map_or(0, |s| s.additions.max(0)));
    let deleted = format_integer(stats.map_or(0, |s| s.deletions.max(0)));
    let scale = diff_stat_scale(width - 18., diff_stat_width(stats) - 18.);
    div()
        .id("project-diff-stat")
        .debug_selector(|| "project-diff-stat".into())
        .role(Role::Button)
        .aria_label(format!(
            "Open Changes: {added} lines added, {deleted} lines deleted"
        ))
        .w(px(width))
        .h(px(24.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .gap(px(6.))
        .px_1p5()
        .rounded_md()
        .whitespace_nowrap()
        .text_size(px(10. * scale))
        .font_weight(FontWeight::MEDIUM)
        .font_features(tabular())
        .child(
            div()
                .text_color(gpui::rgb(DIFF_ADD))
                .child(format!("+{added}")),
        )
        .child(
            div()
                .text_color(gpui::rgb(DIFF_DEL))
                .child(format!("-{deleted}")),
        )
}

struct TabDrag {
    tab: RightTab,
    origin: Pixels,
    offset: Pixels,
    active: bool,
}

/// The title-bar strip observes the panel so selection and drag motion
/// repaint independently of the cached pane and session tabs.
pub(crate) struct ProjectTabs {
    panel: WeakEntity<RightPanel>,
    _subscription: Subscription,
}

impl Render for ProjectTabs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.panel
            .update(cx, |panel, cx| panel.tab_strip(window, cx))
            .unwrap_or_else(|_| div().into_any_element())
    }
}

pub(crate) struct RightPanel {
    pub tabs: Entity<ProjectTabs>,
    pub explorer: Entity<Explorer>,
    pub changes: Entity<Changes>,
    pub history: Entity<History>,
    focus: FocusHandle,
    tab: RightTab,
    order: Vec<RightTab>,
    shown: bool,
    sidebar_tab: Option<RightTab>,
    /// Whether the Terminal tab follows the others; the shell draws its pane.
    terminal_tab: bool,
    /// Classic reserves the title bar's height; rounded content starts at
    /// the card's top edge, below the window-wide title bar.
    top_inset: f32,
    tab_width: f32,
    drag: Option<TabDrag>,
    reveal: Option<TabReveal>,
    skip_click: bool,
    tab_bounds: Vec<crate::popover::AnchorSlot>,
    /// The project whose panels show, and the app snapshot they render from.
    project: String,
    scripts: Vec<ScriptTab>,
    /// None until the shell first builds it; panels never render without it.
    app: Option<serde_json::Value>,
    _subscriptions: Vec<Subscription>,
}
impl EventEmitter<PanelEvent> for RightPanel {}
impl Focusable for RightPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl RightPanel {
    pub(crate) fn new(
        git: Entity<ProjectGit>,
        tab: RightTab,
        order: Vec<RightTab>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let explorer = cx.new(|cx| Explorer::new(git.clone(), window, cx));
        let changes = cx.new(|cx| Changes::new(git.clone(), window, cx));
        let history = cx.new(|cx| History::new(git.clone(), window, cx));
        let mut subscriptions = vec![
            cx.observe(&git, |this, git, cx| {
                let project = git.read(cx).project().to_string();
                this.set_project(project, cx);
                cx.notify()
            }),
            cx.subscribe(&git, |_, _, _: &GitChanged, cx| cx.notify()),
        ];
        let project = git.read(cx).project().to_string();
        if let Some(registry) = crate::panels::get(cx) {
            subscriptions.push(cx.subscribe(&registry, |this, _, _: &RegistryChanged, cx| {
                this.sync_scripts(cx)
            }));
            subscriptions.push(cx.subscribe(
                &registry,
                |this, _, event: &crate::panels::PanelEvent, cx| {
                    for script in &this.scripts {
                        if let Some((view, _)) = &script.view {
                            view.update(cx, |v, cx| v.deliver(event.name, &event.payload, cx));
                        }
                    }
                },
            ));
        }
        let panel = cx.entity();
        let tabs = cx.new(|cx| ProjectTabs {
            panel: panel.downgrade(),
            _subscription: cx.observe(&panel, |_, _, cx| cx.notify()),
        });
        Self {
            tabs,
            focus: cx.focus_handle(),
            explorer,
            changes,
            history,
            tab,
            order: RightTab::normalize(&order),
            shown: false,
            sidebar_tab: None,
            terminal_tab: false,
            top_inset: 0.,
            tab_width: tabs_width(3),
            drag: None,
            reveal: None,
            skip_click: false,
            tab_bounds: (0..4).map(|_| Default::default()).collect(),
            project: String::new(),
            scripts: Vec::new(),
            app: None,
            _subscriptions: subscriptions,
        }
        .with_project(project, cx)
    }
    fn with_project(mut self, project: String, cx: &mut Context<Self>) -> Self {
        self.set_project(project, cx);
        self
    }
    fn set_project(&mut self, project: String, cx: &mut Context<Self>) {
        if self.project == project {
            return;
        }
        self.project = project;
        if let Some(registry) = crate::panels::get(cx) {
            let project = self.project.clone();
            registry.update(cx, |r, cx| r.add_project(&project, cx));
        }
        self.sync_scripts(cx);
    }
    /// Matches the script tabs to the registry, keeping open views whose
    /// file is unchanged and reloading edited ones in place.
    fn sync_scripts(&mut self, cx: &mut Context<Self>) {
        let Some(registry) = crate::panels::get(cx) else {
            return;
        };
        let defs: Vec<_> = registry
            .read(cx)
            .panels_for(&self.project)
            .into_iter()
            .cloned()
            .collect();
        let mut old = std::mem::take(&mut self.scripts);
        for def in defs {
            let mut tab = match old.iter().position(|t| t.id == def.id) {
                Some(ix) => old.remove(ix),
                None => ScriptTab {
                    id: def.id,
                    title: "",
                    icon: def.icon,
                    element_id: intern(&format!("right-panel-tab-panel-{}", def.id.0)),
                    revision: def.revision,
                    view: None,
                },
            };
            tab.title = intern(&def.title);
            tab.icon = def.icon;
            if tab.revision != def.revision {
                tab.revision = def.revision;
                if let Some((view, _)) = &tab.view {
                    view.update(cx, |v, cx| v.reload(def.document(), cx));
                }
            }
            self.scripts.push(tab);
        }
        if let RightTab::Script(id) = self.tab
            && !self.scripts.iter().any(|t| t.id == id)
        {
            self.select(RightTab::Files, cx);
        }
        let count = self.tabs().len();
        if self.tab_bounds.len() < count {
            self.tab_bounds.resize_with(count, Default::default);
        }
        self.sync_panes(cx);
        cx.notify();
    }
    /// Updates the snapshot open panels render from.
    pub(crate) fn set_app(&mut self, app: serde_json::Value, cx: &mut Context<Self>) {
        if self.app.as_ref() == Some(&app) {
            return;
        }
        for script in &self.scripts {
            if let Some((view, _)) = &script.view {
                view.update(cx, |v, cx| v.set_app(&app, cx));
            }
        }
        let first = self.app.is_none();
        self.app = Some(app);
        // The selected panel was waiting for its first snapshot.
        if first && let RightTab::Script(id) = self.tab {
            self.script_view(id, cx);
            self.sync_panes(cx);
            cx.notify();
        }
    }
    /// Whether a Luau panel is on screen, so its snapshot is worth building.
    pub(crate) fn showing_script(&self) -> bool {
        let tab = self.sidebar_tab.unwrap_or(self.tab);
        (self.shown || self.sidebar_tab.is_some()) && matches!(tab, RightTab::Script(_))
    }
    /// Panels by title and file name, for `host.select_tab`.
    pub(crate) fn find_tab(&self, name: &str, cx: &App) -> Option<RightTab> {
        let name = name.trim().to_lowercase();
        match name.as_str() {
            "explorer" | "files" => return Some(RightTab::Files),
            "history" | "graph" => return Some(RightTab::Graph),
            "changes" => return Some(RightTab::Changes),
            "terminal" if self.terminal_tab => return Some(RightTab::Terminal),
            _ => {}
        }
        let registry = crate::panels::get(cx)?;
        let registry = registry.read(cx);
        self.scripts
            .iter()
            .find(|t| {
                t.title.to_lowercase() == name
                    || registry
                        .panel(t.id)
                        .is_some_and(|d| d.stem().to_lowercase() == name)
            })
            .map(|t| RightTab::Script(t.id))
    }
    #[cfg(test)]
    pub(crate) fn script_view_for_test(&self, id: PanelId) -> Option<Entity<VisualView>> {
        self.scripts
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.view.as_ref().map(|(v, _)| v.clone()))
    }
    /// The panel's view, created on first use.
    fn script_view(&mut self, id: PanelId, cx: &mut Context<Self>) -> Option<Entity<VisualView>> {
        let ix = self.scripts.iter().position(|t| t.id == id)?;
        if let Some((view, _)) = &self.scripts[ix].view {
            return Some(view.clone());
        }
        let app = self.app.clone()?;
        let registry = crate::panels::get(cx)?;
        let document = registry.read(cx).panel(id)?.document();
        let slots: std::collections::HashMap<SlotName, gpui::AnyView> = [
            (SlotName::Explorer, self.explorer.clone().into()),
            (SlotName::Changes, self.changes.clone().into()),
            (SlotName::History, self.history.clone().into()),
        ]
        .into_iter()
        .collect();
        let view = cx.new(|cx| VisualView::new_panel(document, app, slots, cx));
        let subscriptions = [
            cx.subscribe(&view, |this, _, _: &VisualChanged, cx| {
                this.sync_panes(cx);
                cx.notify();
            }),
            cx.subscribe(&view, move |_, _, event: &PanelActions, cx| {
                cx.emit(PanelEvent::Actions(id, event.0.clone()));
            }),
        ];
        // Actions from the first render were emitted before anyone listened.
        self.scripts[ix].view = Some((view.clone(), subscriptions));
        self.start_refresh(id, &view, cx);
        Some(view)
    }
    /// Renders a panel again on its declared interval while it exists. The
    /// interval is read each tick, so a reloaded file can add, change or
    /// drop it.
    fn start_refresh(&mut self, id: PanelId, view: &Entity<VisualView>, cx: &mut Context<Self>) {
        let weak = view.downgrade();
        cx.spawn(async move |this, cx| {
            loop {
                let Ok(interval) = weak.read_with(cx, |v, _| v.refresh_interval()) else {
                    break;
                };
                cx.background_executor()
                    .timer(interval.unwrap_or(std::time::Duration::from_secs(1)))
                    .await;
                if interval.is_none() {
                    continue;
                }
                let Ok(visible) = this.update(cx, |this, _| {
                    this.showing_script() && this.tab == RightTab::Script(id)
                }) else {
                    break;
                };
                if visible && weak.update(cx, |v, cx| v.rerender(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }
    pub(crate) fn tab(&self) -> RightTab {
        self.tab
    }
    /// The tabs in strip order: the saved two, with Terminal between them
    /// when enabled, then the Luau panels.
    pub(crate) fn tabs(&self) -> Vec<RightTab> {
        let mut tabs = self.order.clone();
        if self.terminal_tab {
            tabs.insert(1.min(tabs.len()), RightTab::Terminal);
        }
        tabs.extend(self.scripts.iter().map(|t| RightTab::Script(t.id)));
        tabs
    }
    /// The Luau panel tabs as `(element id, label, icon, tab)`.
    pub(crate) fn script_tabs(&self) -> Vec<(&'static str, &'static str, &'static str, RightTab)> {
        self.scripts
            .iter()
            .map(|t| (t.element_id, t.title, t.icon, RightTab::Script(t.id)))
            .collect()
    }
    /// Adds or removes the Terminal tab; Explorer takes over when it goes.
    pub(crate) fn set_terminal_tab(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.terminal_tab == enabled {
            return;
        }
        self.terminal_tab = enabled;
        if !enabled && self.tab == RightTab::Terminal {
            self.select(RightTab::Files, cx);
        }
        cx.notify();
    }
    pub(crate) fn set_show_excluded(&mut self, show: bool, cx: &mut Context<Self>) {
        self.explorer
            .update(cx, |explorer, cx| explorer.set_show_excluded(show, cx));
    }
    pub(crate) fn top_inset(&self) -> f32 {
        self.top_inset
    }
    pub(crate) fn set_top_inset(&mut self, height: f32, cx: &mut Context<Self>) {
        if self.top_inset != height {
            self.top_inset = height;
            cx.notify();
        }
    }
    pub(crate) fn set_tab_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if self.tab_width != width {
            self.tab_width = width;
            cx.notify();
        }
    }
    /// The panes read and poll only while their tab is on screen.
    pub(crate) fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown == shown {
            return;
        }
        self.shown = shown;
        self.sync_panes(cx);
    }
    pub(crate) fn select(&mut self, tab: RightTab, cx: &mut Context<Self>) {
        if self.tab != tab {
            self.reveal = Some(TabReveal::new(tab, self.tab));
        }
        self.tab = tab;
        if let RightTab::Script(id) = tab {
            self.script_view(id, cx);
        }
        self.sync_panes(cx);
        cx.emit(PanelEvent::Tab(tab));
        cx.notify();
    }
    pub(crate) fn set_sidebar_tab(&mut self, tab: Option<RightTab>, cx: &mut Context<Self>) {
        if self.sidebar_tab != tab {
            self.sidebar_tab = tab;
            self.sync_panes(cx);
        }
    }
    fn sync_panes(&mut self, cx: &mut Context<Self>) {
        let (shown, tab) = match self.sidebar_tab {
            Some(tab) => (true, tab),
            None => (self.shown, self.tab),
        };
        // A panel may place the built-in views as slots; wake the ones it shows.
        let placed = match (shown, tab) {
            (true, RightTab::Script(id)) => self
                .scripts
                .iter()
                .find(|t| t.id == id)
                .and_then(|t| t.view.as_ref())
                .map(|(view, _)| view.read(cx).placed_slots())
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        self.explorer.update(cx, |e, cx| {
            e.set_enabled(
                shown && tab == RightTab::Files || placed.contains(&SlotName::Explorer),
                cx,
            )
        });
        self.changes.update(cx, |c, cx| {
            c.set_enabled(
                shown && tab == RightTab::Changes || placed.contains(&SlotName::Changes),
                cx,
            )
        });
        self.history.update(cx, |h, cx| {
            h.set_enabled(
                shown && tab == RightTab::Graph || placed.contains(&SlotName::History),
                cx,
            )
        });
    }
    pub(crate) fn focus_pane(&self, window: &mut Window, cx: &mut App) {
        let focus = match (self.shown, self.tab) {
            (true, RightTab::Files) => self.explorer.focus_handle(cx),
            (true, RightTab::Graph) => self.history.focus_handle(cx),
            _ => self.focus.clone(),
        };
        window.focus(&focus, cx);
    }
    pub(crate) fn finish_opening(&self, window: &mut Window, cx: &mut App) {
        if self.focus.is_focused(window) {
            self.focus_pane(window, cx);
        }
    }

    fn start_drag(&mut self, tab: RightTab, event: &MouseDownEvent) {
        self.drag = Some(TabDrag {
            tab,
            origin: event.position.x,
            offset: px(0.),
            active: false,
        });
    }
    fn move_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let tabs = self.tabs();
        let Some(drag) = &mut self.drag else {
            return;
        };
        if event.pressed_button != Some(MouseButton::Left) {
            self.finish_drag(cx);
            return;
        }
        drag.offset = event.position.x - drag.origin;
        if !drag.active && drag.offset.abs() < px(4.) {
            return;
        }
        drag.active = true;
        // Swap with a neighbour once the dragged tab's centre crosses its centre.
        // Terminal holds its slot, outside the reorderable tabs.
        let Some(from) = tabs.iter().position(|t| *t == drag.tab) else {
            return;
        };
        let bounds: Vec<Option<Bounds<Pixels>>> = tabs
            .iter()
            .zip(&self.tab_bounds)
            .map(|(tab, slot)| {
                slot.get()
                    .filter(|_| matches!(tab, RightTab::Files | RightTab::Changes | RightTab::Graph))
            })
            .collect();
        if let Some(Some(own)) = bounds.get(from) {
            let centre = own.center().x + drag.offset;
            let target = bounds
                .iter()
                .enumerate()
                .filter(|(ix, _)| *ix != from)
                .find(|(ix, b)| {
                    b.is_some_and(|b| {
                        if *ix > from {
                            centre > b.center().x
                        } else {
                            centre < b.center().x
                        }
                    })
                })
                .map(|(ix, _)| ix);
            if let Some(to) = target
                && let Some(Some(other)) = bounds.get(to)
            {
                // A selected tab includes its label, so moving right crosses
                // the other tabs' widths rather than the selected tab's width.
                let shift = if to > from {
                    other.right() - own.right()
                } else {
                    other.left() - own.left()
                };
                let (a, b) = (
                    self.order.iter().position(|t| *t == tabs[from]),
                    self.order.iter().position(|t| *t == tabs[to]),
                );
                if let (Some(a), Some(b)) = (a, b) {
                    self.order.swap(a, b);
                }
                drag.origin += shift;
                drag.offset -= shift;
            }
        }
        cx.notify();
    }
    fn finish_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.drag.take()
            && drag.active
        {
            self.skip_click = true;
            cx.emit(PanelEvent::Reordered(self.order.clone()));
            cx.notify();
        }
    }

    fn tab_strip(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let progress = self
            .reveal
            .as_ref()
            .and_then(|reveal| reveal.progress(self.tab, window, cx));
        let dragging = self
            .drag
            .as_ref()
            .filter(|d| d.active)
            .map(|d| (d.tab, d.offset));
        let count = self.tabs().len();
        let tabs: Vec<AnyElement> = self
            .tabs()
            .into_iter()
            .enumerate()
            .map(|(ix, tab)| {
                let active = self.tab == tab;
                let (id, path, label) = match tab {
                    RightTab::Files => ("right-panel-tab-Explorer", "file-script", tab.label()),
                    RightTab::Changes => ("right-panel-tab-Changes", "git-branch", tab.label()),
                    RightTab::Graph => ("right-panel-tab-History", "clock", tab.label()),
                    RightTab::Terminal => ("right-panel-tab-Terminal", "terminal", tab.label()),
                    RightTab::Script(id) => self
                        .scripts
                        .iter()
                        .find(|t| t.id == id)
                        .map(|t| (t.element_id, t.icon, t.title))
                        .unwrap_or(("right-panel-tab-panel", crate::panels::DEFAULT_ICON, "Panel")),
                };
                let unfolded = match progress {
                    Some((_, p)) if active => p,
                    Some((closing, p)) if tab == closing => 1. - p,
                    _ if active => 1.,
                    _ => 0.,
                };
                let shift = dragging.filter(|(t, _)| *t == tab).map(|(_, o)| o);
                div()
                    .relative()
                    .flex_shrink_0()
                    .flex()
                    .items_stretch()
                    // Keep the measured slot still while its button follows
                    // the pointer; drag offsets are relative to the slot.
                    .child(self.tab_bounds[ix].probe())
                    .child(
                        tab_button(
                            id,
                            TabLabel::from(label)
                                .max_width(tab_label_width(self.tab_width, count)),
                            path,
                            active,
                            false,
                            unfolded,
                            window,
                        )
                        .relative()
                        .when_some(shift, |d, o| d.left(o))
                        .when(!active && shift.is_some(), |d| {
                            d.opacity(1.).text_color(content(1.))
                        })
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, _| {
                                this.start_drag(tab, event)
                            }),
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                if std::mem::take(&mut this.skip_click) {
                                    return;
                                }
                                this.select(tab, cx);
                                this.focus_pane(window, cx);
                            },
                        )),
                    )
                    .into_any_element()
            })
            .collect();
        div()
            .id("right-panel-tabs")
            .debug_selector(|| "right-panel-tabs".into())
            .role(Role::TabList)
            .aria_label("Project")
            .h(px(TITLE_BAR_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(2.))
            .on_mouse_move(
                cx.listener(|this, event: &MouseMoveEvent, _, cx| this.move_drag(event, cx)),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_drag(cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_drag(cx)),
            )
            .children(tabs)
            .into_any_element()
    }
}

impl RightPanel {
    fn script_pane(&mut self, id: PanelId, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = self.script_view(id, cx) else {
            return div().into_any_element();
        };
        let registry = crate::panels::get(cx);
        let def = registry
            .as_ref()
            .and_then(|r| r.read(cx).panel(id).cloned());
        let untrusted = def.as_ref().is_some_and(|d| {
            matches!(d.scope, crate::panels::Scope::Project(_))
                && !registry
                    .as_ref()
                    .is_some_and(|r| r.read(cx).trusted(&self.project))
        });
        let fills = view.read(cx).fills();
        let title = def.as_ref().map_or("Panel", |d| intern(&d.title));
        let header = pane_header("right-panel-script-header", title)
            .child(div().flex_1().min_w_0().truncate().child(title))
        .when(untrusted, |d| {
            let project = self.project.clone();
            d.child(
                div()
                    .id("trust-project-panels")
                    .role(Role::Button)
                    .aria_label("Trust this project's panels")
                    .px_1p5()
                    .rounded(px(4.))
                    .text_size(px(11.))
                    .text_color(content(0.6))
                    .hover(|s| s.bg(content(0.1)).text_color(content(1.)))
                    .cursor_pointer()
                    .child("Trust project")
                    .on_click(move |_, _, cx| {
                        if let Some(registry) = crate::panels::get(cx) {
                            registry.update(cx, |r, cx| r.set_trusted(&project, true, cx));
                        }
                    }),
            )
        })
        .when_some(def.as_ref().map(|d| d.path.clone()), |d, path| {
            d.child(
                icon_action("right-panel-script-source", "Open panel source", "file-script", true)
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(PanelEvent::Actions(
                            id,
                            vec![PanelAction::OpenFile {
                                path: path.to_string_lossy().into_owned(),
                            }],
                        ));
                    })),
            )
        });
        let body = div()
            .id(("right-panel-script", id.0 as usize))
            .flex_1()
            .min_h_0()
            .w_full()
            .p_3()
            .flex()
            .flex_col()
            .when(!fills, |d| d.overflow_y_scroll())
            .child(view);
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .flex()
            .flex_col()
            .child(header)
            .child(body)
            .into_any_element()
    }
}

impl Render for RightPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pane: AnyElement = match self.tab {
            RightTab::Files => self
                .explorer
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
            RightTab::Changes => self
                .changes
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
            RightTab::Graph => self
                .history
                .clone()
                .cached(StyleRefinement::default().flex_1().min_h_0().w_full())
                .into_any_element(),
            // The shell paints the terminal over this pane.
            RightTab::Terminal => div().into_any_element(),
            RightTab::Script(id) => self.script_pane(id, cx),
        };
        div()
            .id("right-panel")
            .track_focus(&self.focus)
            .role(Role::Group)
            .aria_label("Project tools")
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .when(self.top_inset > 0., |d| {
                d.child(
                    div()
                        .h(px(self.top_inset))
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(stroke()),
                )
            })
            .child(pane)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tab_order_keeps_known_tabs_once() {
        assert_eq!(
            RightTab::normalize(&[RightTab::Graph, RightTab::Graph]),
            [RightTab::Graph, RightTab::Files]
        );
        assert_eq!(
            RightTab::normalize(&[RightTab::Changes, RightTab::Graph]),
            [RightTab::Graph, RightTab::Files]
        );
        assert_eq!(format_integer(1234567), "1,234,567");
        assert_eq!(format_integer(12), "12");
    }
}
