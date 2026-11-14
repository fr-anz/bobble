use super::*;
use crate::visual::{SlotName, TextSize};
use serde_json::json;

const COUNTER: &str = r#"
local count = 0
local tab = 1
return {
  title = "Counter",
  icon = "chart-square",
  refresh = 0.2,
  render = function(app)
    return ui.column({
      ui.text(`{app.project.name}: {count}`, { size = "title" }),
      ui.button("Add", function() count += 1 end),
      ui.tabs({ "One", "Two" }, tab, function(i) tab = i end),
      ui.text(`tab {tab}`),
    })
  end,
}
"#;

fn app() -> Value {
    json!({"project": {"name": "demo"}, "sessions": []})
}

fn first_text(node: &Node) -> String {
    let Node::Column { children, .. } = node else {
        panic!("expected a column: {node:?}")
    };
    let Node::Text { text, .. } = &children[0] else {
        panic!("expected text")
    };
    text.clone()
}

fn handler(node: &Node, index: usize) -> String {
    let Node::Column { children, .. } = node else {
        panic!()
    };
    match &children[index] {
        Node::Button { handler, .. } | Node::Tabs { handler, .. } => handler.clone(),
        other => panic!("no handler at {index}: {other:?}"),
    }
}

#[test]
fn renders_and_keeps_state_between_events() {
    let mut panel = PanelRuntime::new("counter.luau", COUNTER).unwrap();
    assert_eq!(panel.meta().title.as_deref(), Some("Counter"));
    assert_eq!(panel.meta().icon.as_deref(), Some("chart-square"));
    // Refresh is clamped to the minimum.
    assert_eq!(panel.meta().refresh, Some(MIN_REFRESH));
    let tree = panel.render(&app()).unwrap();
    assert_eq!(first_text(&tree), "demo: 0");
    let Node::Column { children, .. } = &tree else {
        panic!()
    };
    assert!(matches!(
        children[0],
        Node::Text {
            size: TextSize::Title,
            ..
        }
    ));
    let add = handler(&tree, 1);
    let tree = panel.dispatch(&add, Value::Null, &app()).unwrap();
    assert_eq!(first_text(&tree), "demo: 1");
    // Choice indexes are 0-based on the wire and 1-based in Luau.
    let tabs = handler(&tree, 2);
    let tree = panel.dispatch(&tabs, json!(1), &app()).unwrap();
    let Node::Column { children, .. } = &tree else {
        panic!()
    };
    assert!(matches!(&children[3], Node::Text { text, .. } if text == "tab 2"));
    assert!(matches!(&children[2], Node::Tabs { selected: 1, .. }));
}

#[test]
fn stale_and_invalid_events_are_rejected() {
    let mut panel = PanelRuntime::new("counter.luau", COUNTER).unwrap();
    let tree = panel.render(&app()).unwrap();
    let add = handler(&tree, 1);
    let tabs = handler(&tree, 2);
    assert!(panel.dispatch(&tabs, json!(5), &app()).is_err());
    assert!(panel.dispatch(&add, json!(true), &app()).is_err());
    panel.dispatch(&add, Value::Null, &app()).unwrap();
    // The previous render's handler ids are gone.
    assert!(panel.dispatch(&add, Value::Null, &app()).is_err());
}

#[test]
fn snapshot_is_read_only_and_globals_are_frozen() {
    let mut panel = PanelRuntime::new(
        "write.luau",
        r#"return { render = function(app) app.project.name = "x"; return ui.text("no") end }"#,
    )
    .unwrap();
    assert!(panel.render(&app()).is_err());
    let mut panel = PanelRuntime::new(
        "globals.luau",
        r#"return { render = function() string.upper = nil; return ui.text("no") end }"#,
    )
    .unwrap();
    assert!(panel.render(&app()).is_err());
    let mut panel = PanelRuntime::new(
        "ui.luau",
        r#"return { render = function() ui.text = nil; return ui.text("no") end }"#,
    )
    .unwrap();
    assert!(panel.render(&app()).is_err());
}

#[test]
fn has_no_module_loading_or_os() {
    for global in ["require", "loadstring", "getfenv", "setfenv", "os", "io", "debug"] {
        let source = format!(
            "return {{ render = function() return ui.text(tostring({global} == nil)) end }}"
        );
        let mut panel = PanelRuntime::new("probe.luau", &source).unwrap();
        let tree = panel.render(&app()).unwrap();
        assert!(
            matches!(&tree, Node::Text { text, .. } if text == "true"),
            "{global} should be absent"
        );
    }
}

#[test]
fn long_running_code_is_interrupted() {
    let error = PanelRuntime::new("loop.luau", "while true do end").err().unwrap();
    assert!(error.contains("time budget"), "{error}");
    let mut panel = PanelRuntime::new(
        "busy.luau",
        "return { render = function() while true do end end }",
    )
    .unwrap();
    let error = panel.render(&app()).unwrap_err();
    assert!(error.contains("time budget"), "{error}");
    // The panel stays usable after an interrupted call.
    let mut panel = PanelRuntime::new(
        "maybe.luau",
        "local n = 0\nreturn { render = function() n += 1; if n == 1 then while true do end end; return ui.text('ok') end }",
    )
    .unwrap();
    assert!(panel.render(&app()).is_err());
    assert!(panel.render(&app()).is_ok());
}

#[test]
fn memory_is_capped() {
    let mut panel = PanelRuntime::new(
        "memory.luau",
        "return { render = function() local t = {} for i = 1, 1e8 do t[i] = string.rep('x', 1024) end return ui.text('x') end }",
    )
    .unwrap();
    let error = panel.render(&app()).unwrap_err();
    assert!(
        error.contains("memory") || error.contains("time budget"),
        "{error}"
    );
}

#[test]
fn slots_are_for_panels_only() {
    let mut panel = PanelRuntime::new(
        "files.luau",
        r#"return { render = function() return ui.column({ ui.text("Files"), ui.slot("explorer") }) end }"#,
    )
    .unwrap();
    let tree = panel.render(&app()).unwrap();
    assert_eq!(tree.slots(), [SlotName::Explorer]);
    assert!(tree.validate().is_err());
    let mut twice = PanelRuntime::new(
        "twice.luau",
        r#"return { render = function() return ui.column({ ui.slot("history"), ui.slot("history") }) end }"#,
    )
    .unwrap();
    assert!(twice.render(&app()).is_err());
    let mut unknown = PanelRuntime::new(
        "unknown.luau",
        r#"return { render = function() return ui.slot("terminal") end }"#,
    )
    .unwrap();
    assert!(unknown.render(&app()).is_err());
}

#[test]
fn trees_must_be_plain_data() {
    for (name, body) in [
        ("function.luau", "{ kind = 'text', text = print, size = 'body', color = 'ink' }"),
        ("mixed.luau", "{ kind = 'column', gap = 1, children = { ui.text('a'), extra = 1 } }"),
        ("gap.luau", "{ kind = 'column', gap = 1, children = { [1] = ui.text('a'), [3] = ui.text('b') } }"),
        ("nan.luau", "ui.gauge('x', 0/0)"),
        ("unknown.luau", "{ kind = 'iframe' }"),
    ] {
        let source = format!("return {{ render = function() return {body} end }}");
        let mut panel = PanelRuntime::new(name, &source).unwrap();
        assert!(panel.render(&app()).is_err(), "{name} should fail");
    }
}

#[test]
fn bad_programs_report_errors() {
    assert!(PanelRuntime::new("empty.luau", "  ").is_err());
    assert!(PanelRuntime::new("syntax.luau", "return {").is_err());
    let error = PanelRuntime::new("shape.luau", "return 1").err().unwrap();
    assert!(error.contains("must return a table"), "{error}");
    let error = PanelRuntime::new("norender.luau", "return {}").err().unwrap();
    assert!(error.contains("render"), "{error}");
}

#[test]
fn actions_are_queued_and_checked() {
    let mut panel = PanelRuntime::new(
        "actions.luau",
        r#"
return {
  render = function(app)
    return ui.button("Go", function()
      host.notify("hi")
      host.open_file("src/main.rs")
      host.send(7, "continue")
      host.new_chat()
    end)
  end,
}"#,
    )
    .unwrap();
    let tree = panel.render(&app()).unwrap();
    assert!(panel.take_actions().is_empty());
    let Node::Button { handler, .. } = tree else {
        panic!()
    };
    panel.dispatch(&handler, Value::Null, &app()).unwrap();
    let actions = panel.take_actions();
    assert_eq!(
        actions,
        [
            PanelAction::Notify {
                message: "hi".into()
            },
            PanelAction::OpenFile {
                path: "src/main.rs".into()
            },
            PanelAction::Send {
                session: 7,
                message: "continue".into()
            },
            PanelAction::NewChat {
                prompt: String::new()
            },
        ]
    );
    assert!(actions[2].starts_work() && !actions[0].starts_work());
    assert!(panel.take_actions().is_empty());
}

#[test]
fn events_rerender_only_when_handled() {
    let mut panel = PanelRuntime::new(
        "events.luau",
        r#"
local finished = 0
return {
  on = { turn_finished = function(event, app) finished += event.count end },
  render = function() return ui.text(`finished {finished}`) end,
}"#,
    )
    .unwrap();
    panel.render(&app()).unwrap();
    assert!(panel.event("approval", &json!({}), &app()).unwrap().is_none());
    let tree = panel
        .event("turn_finished", &json!({"count": 2}), &app())
        .unwrap()
        .unwrap();
    assert!(matches!(tree, Node::Text { text, .. } if text == "finished 2"));
}

#[test]
fn snapshot_lists_are_one_based() {
    let mut panel = PanelRuntime::new(
        "list.luau",
        r#"return { render = function(app) return ui.text(`{#app.sessions} {app.sessions[1].title}`) end }"#,
    )
    .unwrap();
    let tree = panel
        .render(&json!({"sessions": [{"title": "a"}, {"title": "b"}]}))
        .unwrap();
    assert!(matches!(tree, Node::Text { text, .. } if text == "2 a"));
}

const POLICY: &str = r#"
return {
  approval = function(request)
    if string.find(request.title, "Read", 1, true) then return "allow" end
    if string.find(request.detail, "rm -rf", 1, true) then return "deny" end
    if request.title == "Edit" then return "session" end
    return nil
  end,
  turn_finished = function(event)
    host.notify(`done {event.session}`)
  end,
}"#;

fn request(title: &str, detail: &str, can_remember: bool) -> ApprovalRequest {
    ApprovalRequest {
        session: 1,
        provider: "claude".into(),
        title: title.into(),
        detail: detail.into(),
        cwd: "/tmp".into(),
        project: "/tmp".into(),
        can_remember,
    }
}

#[test]
fn approval_hooks_decide_or_defer() {
    let mut hook = HookRuntime::new("policy.luau", POLICY).unwrap();
    assert!(hook.handles("approval") && hook.handles("turn_finished"));
    assert_eq!(
        hook.approval(&request("Read file", "", false)).unwrap(),
        Some(HookDecision::Allow)
    );
    assert_eq!(
        hook.approval(&request("Bash", "rm -rf /", false)).unwrap(),
        Some(HookDecision::Deny)
    );
    assert_eq!(
        hook.approval(&request("Edit", "", true)).unwrap(),
        Some(HookDecision::Session)
    );
    // Without session memory, "session" means a single allow.
    assert_eq!(
        hook.approval(&request("Edit", "", false)).unwrap(),
        Some(HookDecision::Allow)
    );
    assert_eq!(hook.approval(&request("Bash", "ls", false)).unwrap(), None);
    hook.call("turn_finished", &json!({"session": 3})).unwrap();
    assert_eq!(
        hook.take_actions(),
        [PanelAction::Notify {
            message: "done 3".into()
        }]
    );
}

#[test]
fn hook_files_are_checked() {
    assert!(HookRuntime::new("bad.luau", "return { approve = function() end }").is_err());
    assert!(HookRuntime::new("bad.luau", "return { approval = 1 }").is_err());
    let mut hook =
        HookRuntime::new("odd.luau", "return { approval = function() return 'maybe' end }")
            .unwrap();
    assert!(hook.approval(&request("x", "", false)).is_err());
}

#[test]
fn decisions_combine_with_deny_first() {
    use HookDecision::*;
    assert_eq!(combine([]), None);
    assert_eq!(combine([Allow, Deny]), Some(Deny));
    assert_eq!(combine([Session, Allow]), Some(Allow));
    assert_eq!(combine([Session]), Some(Session));
}

#[test]
fn discovers_luau_files_in_order() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["b.luau", "a.luau", "notes.md"] {
        std::fs::write(dir.path().join(name), "x").unwrap();
    }
    let found: Vec<_> = discover(dir.path())
        .into_iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(found, ["a.luau", "b.luau"]);
    assert!(discover(&dir.path().join("missing")).is_empty());
}
