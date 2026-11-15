//! The shell's side of Luau panels and hooks: the app snapshot panels read,
//! the events they and hooks observe, and running the actions they ask for.
use super::*;
use crate::panels::{HookActions, PanelId, Problem};
use monocode_core::panel::PanelAction;
use serde_json::{Value, json};

/// Board cards are read from the database, so the snapshot rereads them at
/// most this often.
const BOARD_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
/// Animations redraw the shell every frame, so the snapshot is rebuilt at
/// most this often; a skipped rebuild is retried when the wait ends.
const SNAPSHOT_EVERY: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Default)]
pub(super) struct State {
    board: Option<(std::time::Instant, String, Vec<Value>)>,
    _hooks: Option<Subscription>,
    _problems: Option<Subscription>,
    _composer: Option<Subscription>,
    snapshot_at: Option<std::time::Instant>,
    retry: Option<gpui::Task<()>>,
}

impl Shell {
    /// Runs actions queued by hooks; they act on the current project.
    pub(super) fn observe_hooks(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.luau._hooks.is_some() {
            return;
        }
        let Some(registry) = crate::panels::get(cx) else {
            return;
        };
        self.luau._hooks = Some(cx.subscribe_in(
            &registry,
            window,
            |this, _, event: &HookActions, window, cx| {
                this.run_panel_actions("Hook", None, event.0.clone(), window, cx)
            },
        ));
        self.luau._problems = Some(cx.subscribe(&registry, |this, _, problem: &Problem, cx| {
            this.panel_notice(&problem.source, problem.message.clone(), cx)
        }));
        self.luau._composer = crate::composer_panels::get(cx).map(|host| {
            cx.subscribe_in(
                &host,
                window,
                |this, _, event: &crate::composer_panels::ComposerPanelActions, window, cx| {
                    this.panel_actions(event.0, event.1.clone(), window, cx)
                },
            )
        });
    }

    /// Tells hooks and panels about an app event.
    pub(super) fn panel_event(
        &mut self,
        name: &'static str,
        payload: Value,
        cx: &mut Context<Self>,
    ) {
        if let Some(registry) = crate::panels::get(cx) {
            registry.update(cx, |r, cx| r.emit_event(name, payload, cx));
        }
    }

    /// A Luau panel's actions, checked against its scope and trust.
    pub(super) fn panel_actions(
        &mut self,
        id: PanelId,
        actions: Vec<PanelAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(registry) = crate::panels::get(cx) else {
            return;
        };
        let Some(def) = registry.read(cx).panel(id).cloned() else {
            return;
        };
        let (allowed, refused): (Vec<_>, Vec<_>) = actions
            .into_iter()
            .partition(|a| registry.read(cx).allows(&def, a));
        if !refused.is_empty() {
            self.panel_notice(
                &def.title,
                "This project's panels can start agent work after you trust the project.".into(),
                cx,
            );
        }
        self.run_panel_actions(&def.title, Some(&def), allowed, window, cx);
    }

    fn run_panel_actions(
        &mut self,
        source: &str,
        def: Option<&crate::panels::PanelDef>,
        actions: Vec<PanelAction>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let project = self.workspace.project.clone();
        for action in actions {
            match action {
                PanelAction::Notify { message } => self.panel_notice(source, message, cx),
                PanelAction::Copy { text } => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
                PanelAction::OpenFile { path } => {
                    // A panel's own source opens by its absolute path; anything
                    // else must stay inside the project.
                    let own = def.is_some_and(|d| d.path == std::path::Path::new(&path));
                    let target = if own {
                        let file = std::path::Path::new(&path);
                        file.parent().zip(file.file_name()).map(|(dir, name)| {
                            (
                                dir.to_string_lossy().into_owned(),
                                name.to_string_lossy().into_owned(),
                            )
                        })
                    } else {
                        project_file(&project, &path).map(|p| (project.clone(), p))
                    };
                    match target {
                        Some((cwd, path)) => self.open_review(
                            crate::review::ReviewTab::File { cwd, path },
                            window,
                            cx,
                        ),
                        None => self.panel_notice(
                            source,
                            format!("Cannot open {path}: files must be inside the project"),
                            cx,
                        ),
                    }
                }
                PanelAction::OpenSession { session } => {
                    if self.panel_session(session, &project, cx) {
                        self.open(session, window, cx);
                    } else {
                        self.panel_notice(source, "No such session in this project".into(), cx);
                    }
                }
                PanelAction::Send { session, message } => {
                    if let Err(error) = self.panel_send(session, &project, message, cx) {
                        self.panel_notice(source, error, cx);
                    }
                }
                PanelAction::NewChat { prompt } => self.panel_new_chat(prompt, cx),
                PanelAction::SelectTab { tab } => {
                    self.ensure_right_panel(window, cx);
                    let Some(panel) = self.right_panel.clone() else {
                        continue;
                    };
                    match panel.read(cx).find_tab(&tab, cx) {
                        Some(found) => {
                            if !self.workspace.right_panel_open {
                                self.toggle_right_panel(window, cx);
                            }
                            panel.update(cx, |p, cx| p.select(found, cx));
                        }
                        None => self.panel_notice(source, format!("No tab named {tab}"), cx),
                    }
                }
            }
        }
    }

    fn panel_session(&self, id: i64, project: &str, cx: &App) -> bool {
        (self.sessions.contains_key(&id) || self.session_summaries.contains_key(&id))
            && monocode_core::files::path_key(&self.session_metadata(id, cx).summary.cwd)
                == monocode_core::files::path_key(project)
    }

    /// Like the app API's `sessions.send`: only to an idle session without a draft.
    fn panel_send(
        &mut self,
        id: i64,
        project: &str,
        message: String,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if message.trim().is_empty() {
            return Err("Cannot send an empty message".into());
        }
        if !self.panel_session(id, project, cx) {
            return Err("No such session in this project".into());
        }
        let view = self.ensure_session(id, cx).ok_or("Session not found")?;
        view.update(cx, |v, cx| v.load(cx));
        if !view.read(cx).ready_for_external(cx) || view.read(cx).has_saved_draft() {
            return Err("That session is busy, loading, or has an unsent draft".into());
        }
        view.update(cx, |v, cx| {
            v.install_transfer(v.settings.clone(), Some(message), cx)
        });
        Ok(())
    }

    /// Opens a new chat in the current project with `prompt` in its
    /// composer, unsent.
    fn panel_new_chat(&mut self, prompt: String, cx: &mut Context<Self>) {
        let before: std::collections::HashSet<i64> = self.sessions.keys().copied().collect();
        self.new_chat(cx);
        if prompt.trim().is_empty() {
            return;
        }
        // The session is created in the background; fill its composer once
        // it exists.
        cx.spawn(async move |this, cx| {
            for _ in 0..50 {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(50))
                    .await;
                let done = this
                    .update(cx, |this, cx| {
                        let Some((_, view)) =
                            this.sessions.iter().find(|(id, _)| !before.contains(id))
                        else {
                            return false;
                        };
                        let view = view.clone();
                        view.update(cx, |v, cx| v.save_app_draft(prompt.clone(), cx));
                        true
                    })
                    .unwrap_or(true);
                if done {
                    break;
                }
            }
        })
        .detach();
    }

    pub(super) fn panel_notice(&mut self, source: &str, message: String, cx: &mut Context<Self>) {
        self.notification_state.panel.update(cx, |notice, cx| {
            notice.push(source.to_string(), message, cx)
        });
        cx.notify();
    }

    /// Rebuilds the snapshot while a panel is on screen and hands it over
    /// when it changed.
    pub(super) fn sync_panel_snapshot(&mut self, cx: &mut Context<Self>) {
        let composer = crate::composer_panels::get(cx);
        if let Some(composer) = &composer {
            let project = self.workspace.project.clone();
            composer.update(cx, |c, cx| c.set_project(&project, cx));
        }
        let panel = self
            .right_panel
            .clone()
            .filter(|p| p.read(cx).showing_script());
        let composer = composer.filter(|c| c.read(cx).wanted());
        if panel.is_none() && composer.is_none() {
            return;
        }
        let now = std::time::Instant::now();
        if let Some(at) = self.luau.snapshot_at
            && now < at + SNAPSHOT_EVERY
        {
            if self.luau.retry.is_none() {
                let wait = at + SNAPSHOT_EVERY - now;
                self.luau.retry = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(wait).await;
                    let _ = this.update(cx, |this, cx| {
                        this.luau.retry = None;
                        cx.notify();
                    });
                }));
            }
            return;
        }
        self.luau.snapshot_at = Some(now);
        // Each receiver skips a snapshot it already has.
        let snapshot = self.panel_snapshot(cx);
        if let Some(composer) = composer {
            composer.update(cx, |c, cx| c.set_app(&snapshot, cx));
        }
        if let Some(panel) = panel {
            panel.update(cx, |p, cx| p.set_app(snapshot, cx));
        }
    }

    fn panel_snapshot(&mut self, cx: &mut Context<Self>) -> Value {
        let project = self.workspace.project.clone();
        let key = monocode_core::files::path_key(&project);
        let sessions: Vec<Value> = self
            .order
            .iter()
            .filter_map(|id| {
                let metadata = self.session_metadata(*id, cx);
                if monocode_core::files::path_key(&metadata.summary.cwd) != key
                    || self
                        .minions
                        .as_ref()
                        .is_some_and(|m| m.read(cx).hidden_session(*id))
                    || self.orchestration_worker(*id)
                {
                    return None;
                }
                let live = self.sessions.get(id).map(|v| v.read(cx));
                Some(json!({
                    "id": id,
                    "title": metadata.summary.title,
                    "provider": metadata.summary.provider,
                    "model": live
                        .and_then(|v| v.settings.model.clone())
                        .or_else(|| self.session_details.get(id).and_then(|m| m.model.clone())),
                    "busy": metadata.busy,
                    "needs_approval": live.is_some_and(|v| v.needs_approval()),
                    "unread": self.unseen_finished.contains(id),
                    "archived": metadata.summary.archived,
                    "pinned": metadata.summary.pinned,
                    "updated_at": metadata.summary.updated_at,
                    "active": self.workspace.active == Some(*id),
                }))
            })
            .take(500)
            .collect();
        let git = self.git.read(cx);
        let stats = git
            .stats
            .as_ref()
            .map(|s| json!({"files": s.files, "additions": s.additions, "deletions": s.deletions}));
        let root = git.root.clone();
        let board = self.panel_board(&project, cx);
        json!({
            "project": {
                "path": project,
                "name": self.project_label(&project),
                "root": root,
                "trusted": crate::panels::get(cx).is_some_and(|r| r.read(cx).trusted(&project)),
            },
            "git": stats,
            "sessions": sessions,
            "board": board,
            "active_session": self.workspace.active,
        })
    }

    fn panel_board(&mut self, project: &str, cx: &mut Context<Self>) -> Vec<Value> {
        if let Some((at, cached_project, cards)) = &self.luau.board
            && cached_project == project
            && at.elapsed() < BOARD_EVERY
        {
            return cards.clone();
        }
        let cards: Vec<Value> = locked(&self.store)
            .map(|store| monocode_core::board::load(&store))
            .unwrap_or_default()
            .into_iter()
            .filter(|c| c.project == project && !c.archived)
            .take(200)
            .map(|card| {
                let state = card.lead.map(|lead| self.lead_state(lead, cx));
                let lane = match monocode_core::board::lane(&card, state) {
                    monocode_core::board::Lane::Backlog => "backlog",
                    monocode_core::board::Lane::InProgress => "in_progress",
                    monocode_core::board::Lane::NeedsReview => "needs_review",
                };
                json!({
                    "id": card.id,
                    "number": card.number,
                    "title": card.title,
                    "lane": lane,
                    "lead": card.lead,
                })
            })
            .collect();
        self.luau.board = Some((
            std::time::Instant::now(),
            project.to_string(),
            cards.clone(),
        ));
        cards
    }
}

/// `path` relative to `project`, if it stays inside it.
fn project_file(project: &str, path: &str) -> Option<String> {
    use std::path::{Component, Path};
    let relative = Path::new(path);
    let relative = if relative.is_absolute() {
        relative.strip_prefix(project).ok()?
    } else {
        relative
    };
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return None;
    }
    Some(relative.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::project_file;

    #[test]
    fn panel_files_stay_inside_the_project() {
        assert_eq!(project_file("/p", "src/a.rs").as_deref(), Some("src/a.rs"));
        assert_eq!(
            project_file("/p", "/p/src/a.rs").as_deref(),
            Some("src/a.rs")
        );
        assert_eq!(project_file("/p", "../etc/passwd"), None);
        assert_eq!(project_file("/p", "src/../../x"), None);
        assert_eq!(project_file("/p", "/etc/passwd"), None);
        assert_eq!(project_file("/p", ""), None);
    }
}

#[cfg(test)]
mod shell_tests {
    use super::*;
    use crate::right_panel::RightTab;
    use gpui::TestAppContext;
    use monocode_core::{panel::HookDecision, store::Store, visual::Node};
    use std::sync::{Arc, Mutex};

    const PANEL: &str = r#"
return {
  title = "Hello",
  icon = "rocket",
  render = function(app)
    return ui.column({
      ui.text(`{#app.sessions} sessions in {app.project.path}`),
      ui.button("Go", function()
        host.select_tab("history")
        host.notify("went")
        host.send(999999, "nope")
      end),
    })
  end,
}"#;
    const HOOK: &str = r#"
return {
  approval = function(r) if r.tool == "Read" then return "allow" end return nil end,
  turn_finished = function(e) host.notify(`done {e.session}`) end,
}"#;

    fn texts(node: &Node, out: &mut Vec<String>) {
        match node {
            Node::Column { children, .. } => children.iter().for_each(|c| texts(c, out)),
            Node::Text { text, .. } => out.push(text.clone()),
            _ => {}
        }
    }
    fn button(node: &Node) -> Option<String> {
        match node {
            Node::Column { children, .. } => children.iter().find_map(button),
            Node::Button { handler, .. } => Some(handler.clone()),
            _ => None,
        }
    }

    #[gpui::test]
    fn user_panels_render_act_and_hooks_answer(cx: &mut TestAppContext) {
        let data = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(data.path().join("panels")).unwrap();
        std::fs::create_dir_all(data.path().join("hooks")).unwrap();
        std::fs::write(data.path().join("panels/hello.luau"), PANEL).unwrap();
        std::fs::write(data.path().join("hooks/policy.luau"), HOOK).unwrap();
        let registry_store = Arc::new(Mutex::new(
            Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let path = data.path().to_path_buf();
        cx.update(|cx| {
            cx.set_reduce_motion(true);
            crate::panels::install_at(registry_store, Some(path), cx);
        });
        let (window, _, b) = crate::shell::tests::window(cx);
        let id = window
            .update(cx, |shell, w, cx| {
                shell.open(b, w, cx);
                shell.ensure_right_panel(w, cx);
                if !shell.workspace.right_panel_open {
                    shell.toggle_right_panel(w, cx);
                }
                let panel = shell.right_panel.clone().unwrap();
                let tabs = panel.read(cx).script_tabs();
                assert_eq!(tabs.len(), 1, "one user panel tab");
                assert_eq!(tabs[0].1, "Hello");
                assert_eq!(tabs[0].2, "rocket");
                let RightTab::Script(id) = tabs[0].3 else {
                    panic!()
                };
                panel.update(cx, |p, cx| p.select(RightTab::Script(id), cx));
                id
            })
            .unwrap();
        // The next frame builds the snapshot the panel waits for.
        cx.refresh().unwrap();
        cx.run_until_parked();
        let view = window
            .update(cx, |shell, _, cx| {
                shell
                    .right_panel
                    .clone()
                    .unwrap()
                    .read(cx)
                    .script_view_for_test(id)
                    .unwrap()
            })
            .unwrap();
        // The snapshot reached the panel: this project's one session.
        let rendered = view.read_with(cx, |v, _| {
            let mut out = Vec::new();
            texts(v.tree().unwrap(), &mut out);
            out
        });
        assert_eq!(rendered, ["1 sessions in /work/monocode"]);
        // Pressing the button runs its actions through the shell.
        let handler = view.read_with(cx, |v, _| button(v.tree().unwrap()).unwrap());
        view.update(cx, |v, cx| v.press(&handler, Value::Null, cx));
        cx.run_until_parked();
        window
            .update(cx, |shell, _, cx| {
                let panel = shell.right_panel.clone().unwrap();
                assert_eq!(panel.read(cx).tab(), RightTab::Graph);
                // Selecting a panel never overwrites the saved built-in tab.
                assert!(shell.workspace.right_panel_tab.saved());
                assert!(shell.notification_state.panel.read(cx).is_visible());
            })
            .unwrap();

        // Hooks: an approval answer, and an observer's queued notice.
        let registry = cx.update(|cx| crate::panels::get(cx).unwrap());
        let request = monocode_core::panel::ApprovalRequest {
            session: b,
            provider: "claude".into(),
            title: "Allow Read?".into(),
            tool: Some("Read".into()),
            detail: "src/main.rs".into(),
            cwd: "/work/monocode".into(),
            project: "/work/monocode".into(),
            can_remember: true,
        };
        let decision = registry.update(cx, |r, cx| r.approval(&request, cx));
        assert_eq!(decision, Some(HookDecision::Allow));
        let other = monocode_core::panel::ApprovalRequest {
            title: "Allow Bash?".into(),
            tool: Some("Bash".into()),
            ..request
        };
        assert_eq!(registry.update(cx, |r, cx| r.approval(&other, cx)), None);
    }

    #[gpui::test]
    fn project_panels_need_trust_to_start_work(cx: &mut TestAppContext) {
        let data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let panels = project.path().join(".placeholder/panels");
        std::fs::create_dir_all(&panels).unwrap();
        std::fs::write(panels.join("team.luau"), PANEL).unwrap();
        let store = Arc::new(Mutex::new(
            Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let path = data.path().to_path_buf();
        let registry = cx.update(|cx| crate::panels::install_at(store, Some(path), cx));
        let root = project.path().to_string_lossy().into_owned();
        registry.update(cx, |r, cx| r.add_project(&root, cx));
        let def = registry.read_with(cx, |r, _| {
            let found = r.panels_for(&root);
            assert_eq!(found.len(), 1);
            found[0].clone()
        });
        let send = PanelAction::Send {
            session: 1,
            message: "go".into(),
        };
        let notify = PanelAction::Notify {
            message: "hi".into(),
        };
        registry.read_with(cx, |r, _| {
            assert!(!r.allows(&def, &send));
            assert!(r.allows(&def, &notify));
        });
        registry.update(cx, |r, cx| r.set_trusted(&root, true, cx));
        registry.read_with(cx, |r, _| assert!(r.allows(&def, &send)));
        // Another project does not see this project's panels.
        registry.read_with(cx, |r, _| assert!(r.panels_for("/elsewhere").is_empty()));
    }
}
