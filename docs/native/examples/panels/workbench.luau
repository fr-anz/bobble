--!strict
-- Workbench: the change summary above one of the app's own views, picked
-- with tabs. Shows how a panel places native views with `ui.slot`.

local view = 1
local views = { "changes", "explorer", "history" }

return {
	title = "Workbench",
	icon = "app-window",

	render = function(app)
		local git = app.git
		local summary = if git
			then ui.row({
				ui.stat("Files", git.files),
				ui.stat("Added", `+{git.additions}`, { color = "positive" }),
				ui.stat("Removed", `-{git.deletions}`, { color = "negative" }),
			}, { gap = 8 })
			else ui.text("No changes yet.", { color = "muted" })

		return ui.column({
			summary,
			ui.tabs({ "Changes", "Files", "History" }, view, function(i)
				view = i
			end),
			-- No height: the view fills the rest of the panel.
			ui.slot(views[view]),
		}, { gap = 10 })
	end,
}
