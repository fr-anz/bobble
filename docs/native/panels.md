# Luau panels and hooks

Bobble can be extended with small [Luau](https://luau.org) programs:

- **Panels** add tabs to the right panel. A panel describes what to show; the app draws it with the same native components as session visuals, in the app's own fonts and colors. A panel can also place the app's own Explorer, Changes and History views.
- **Hooks** run when something happens in the app. An `approval` hook can answer an agent's permission request for you. Other hooks observe events, such as a turn finishing.

Every file reloads when you save it. There is nothing to build or install.

## Where files go

| What | Folder | Trust |
| --- | --- | --- |
| Your panels | `<data>/panels/*.luau` | Always trusted |
| Your hooks | `<data>/hooks/*.luau` | Always trusted |
| A project's panels | `<project>/.placeholder/panels/*.luau` | Can start agent work only after you trust the project |

`<data>` is the app's data folder. To print the exact paths, run:

```sh
placeholder panel paths
```

(`bobble` is the installed command. A source build runs it as `bobble-gpui`.)

Each folder loads at most 32 files, in name order. A project's panels show only while that project is open. Hooks are user-only: a repository cannot install a hook.

## A first panel

Save this as `<data>/panels/hello.luau`:

```lua
local count = 0

return {
  title = "Hello",          -- the tab label; defaults to the file name
  icon = "rocket",          -- see "Icons" below
  render = function(app)
    return ui.column({
      ui.text(`{#app.sessions} sessions in {app.project.name}`, { size = "title" }),
      ui.button(`Clicked {count} times`, function()
        count += 1
      end),
    })
  end,
}
```

A **Hello** tab appears in the right panel. `render` runs again after every control event, whenever the app snapshot changes, after a handled event, and on the panel's `refresh` interval. Locals at the top of the file are the panel's state. They last until the file changes or the app restarts.

### The panel table

| Field | Meaning |
| --- | --- |
| `render(app)` | Required. Returns one component. |
| `title` | Tab label, up to 48 characters. |
| `icon` | Tab icon name. |
| `refresh` | Seconds between re-renders while the panel is visible; at least 1. |
| `placement` | `"right"` (the default) for a right-panel tab, or `"composer"` to show the panel below a new session's composer. Composer panels cannot use slots. |
| `on` | A table of event handlers, such as `on = { turn_finished = function(event, app) ... end }`. A handled event re-renders the panel. |

## The app snapshot

`render` and event handlers receive `app`, a read-only table. Lists are 1-based.

```lua
app.project      -- { path, name, root, trusted }
app.git          -- { files, additions, deletions }, or nil without changes
app.sessions     -- { id, title, provider, model, busy, needs_approval,
                 --   unread, archived, pinned, updated_at, active }
app.board        -- { id, number, title, lane, lead }; lane is "backlog",
                 --   "in_progress" or "needs_review"
app.active_session -- the focused session's id, or nil
```

Sessions are the current project's own chats, newest first, at most 500. The snapshot is rebuilt only while a panel is visible, and a panel re-renders only when it changes.

## Components

Panels use the same component set as [session visuals](visuals.md), written as Luau functions on `ui`. Option tables are optional.

| Component | Signature |
| --- | --- |
| Layout | `ui.column(children, {gap})`, `ui.row(children, {gap})`, `ui.grid(children, {columns, gap})`, `ui.card(children, {gap})` |
| Text | `ui.text(value, {size, color})`, `ui.stat(label, value, {detail, color, delta, trend, better})` |
| Data | `ui.table(columns, rows)`, `ui.chart(type, series, {labels, height})`, `ui.gauge(label, value, {min, max, unit, color})`, `ui.donut(slices, {label, value})`, `ui.heatmap(values, {rows, columns, color})`, `ui.progress(label, value, {max, detail, color})` |
| Actions | `ui.button(label, fn, {disabled})`, `ui.toggle(label, value, fn)` |
| Values | `ui.slider(label, value, min, max, fn, {step})`, `ui.input(label, value, fn, {placeholder})` |
| Choices | `ui.tabs(labels, selected, fn)`, `ui.select(label, options, selected, fn)`. Indexes are 1-based. |
| Drawing | `ui.canvas(shapes, {width, height})` with `ui.rect`, `ui.circle`, `ui.line`, `ui.polyline` and `ui.label` |
| App views | `ui.slot(name, {height})` |

Colors are `ink`, `muted`, `faint`, `accent`, `positive`, `negative`, `warning` and `series1` to `series6`. Text sizes are `caption`, `small`, `body`, `subtitle`, `title` and `hero`. Any other style or option is an error.

A panel tree may hold up to 2048 components, 16 levels deep. The other limits are the same as for visuals.

### App views

`ui.slot("explorer")`, `ui.slot("changes")` and `ui.slot("history")` place the app's own native views inside the panel. Each slot may appear once per render. A slot without `height` fills the space left in the panel, and the panel then stops scrolling. A slot with a `height` (80–4000 pixels) scrolls along with the rest. Only panels can use slots. Agent visuals cannot.

```lua
return {
  title = "Workbench",
  render = function(app)
    return ui.column({
      ui.stat("Changed files", app.git and app.git.files or 0),
      ui.slot("changes"),
    })
  end,
}
```

## Asking the app to do things

A panel cannot change the app directly. Instead it calls functions on `host`. The app runs them after the current call returns, in order, at most 32 per call:

| Function | What happens |
| --- | --- |
| `host.notify(message)` | Shows a short card in the notification stack. |
| `host.open_file(path)` | Opens a file in the project. The path must stay inside the project. |
| `host.open_session(id)` | Opens one of this project's sessions. |
| `host.send(id, message)` | Sends a message to an idle session with no unsent draft. |
| `host.new_chat(prompt)` | Starts a new chat with `prompt` in its composer, unsent. |
| `host.copy(text)` | Copies text to the clipboard. |
| `host.select_tab(name)` | Selects a right-panel tab: `explorer`, `history`, `changes`, `terminal`, or a panel's title or file name. |

`host.send` and `host.new_chat` make agents work, so a project's panels may use them only after you trust the project. Use **Trust project** in the panel's header. The panel's other actions always work. The header's file icon opens the panel's source.

## Hooks

A hook file returns a table of functions named after events:

```lua
return {
  approval = function(request)
    if request.tool == "Read" then
      return "allow"
    end
    return nil -- ask as usual
  end,
  turn_finished = function(event)
    if not event.cancelled then
      host.notify(`{event.title} finished`)
    end
  end,
}
```

| Event | Payload | Result |
| --- | --- | --- |
| `approval` | `{ session, provider, title, tool, detail, cwd, project, can_remember }` | `"allow"`, `"session"` (allow for the rest of the session, where the provider supports it), `"deny"`, `"ask"` or `nil` |
| `turn_started` | `{ session, title, provider, cwd, cancelled }` | ignored |
| `turn_finished` | the same; `cancelled` is true for a stopped turn | ignored |

An unknown event name is a load error. When several hooks answer the same request, any `"deny"` wins, then `"allow"`. A request that no hook answers appears as usual, and panels receive it as an `approval` event. `tool` is filled in when the provider names one, such as `"Bash"` for Claude Code's "Allow Bash?". Generic requests, such as Codex's "Allow this command?", have no tool, so match on `detail` instead.

Hooks may call `host` functions too. Their actions act on the open project.

## Checking files

```sh
placeholder panel check path/to/panel.luau            # render with a sample snapshot
placeholder panel check panel.luau --app snapshot.json # or with your own
placeholder panel hook path/to/hook.luau              # list the events it handles
```

`panel check` loads the panel, renders it, and presses each control once in a fresh VM. It prints the title, icon, refresh interval, slots and control count as JSON, or the first error. In the app, a broken panel shows its error in its tab, and load errors also appear as a notice.

## Sandbox and limits

Each panel and hook runs in its own Luau VM:

- It has the `math`, `string`, `table`, `utf8`, `bit32`, `buffer` and `vector` libraries. `os` has only `clock`, `date`, `difftime` and `time`.
- There is no `require`, `loadstring`, `getfenv`, `setfenv`, `io`, `debug` or `collectgarbage`. It has no filesystem, network, process or timer access.
- Globals and the `ui` and `host` tables are frozen. The `app` snapshot and event payloads are read-only.
- Each VM has 32 MiB of memory. Each call has a 60 ms budget, because panels run on the UI thread. A call that runs over stops with an error, and the next call starts fresh.
- A source file is at most 256 KiB.

## Examples

- [`panels/sessions.luau`](examples/panels/sessions.luau): counts and shortcuts for the project's sessions.
- [`panels/workbench.luau`](examples/panels/workbench.luau): a change summary above a switchable native view.
- [`panels/board.luau`](examples/panels/board.luau): the board's lanes, refreshed every 5 seconds.
- [`hooks/approvals.luau`](examples/hooks/approvals.luau): an approval policy and a completion notice.

Every example is loaded, rendered and exercised by the test suite.
