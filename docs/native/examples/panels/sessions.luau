--!strict
-- Sessions: what every chat in this project is doing, with shortcuts.
-- Copy to <data>/panels/sessions.luau to try it.

local filter = 1 -- 1 all, 2 working, 3 needs you

local function state(s): string
	if s.needs_approval then
		return "Needs approval"
	elseif s.busy then
		return "Working"
	elseif s.unread then
		return "Finished"
	end
	return "Idle"
end

return {
	title = "Sessions",
	icon = "message-multiple",

	render = function(app)
		local working, waiting, unread = 0, 0, 0
		local shown = {}
		for _, s in app.sessions do
			if s.archived then
				continue
			end
			if s.busy then
				working += 1
			end
			if s.needs_approval then
				waiting += 1
			end
			if s.unread then
				unread += 1
			end
			local keep = filter == 1 or (filter == 2 and s.busy) or (filter == 3 and (s.needs_approval or s.unread))
			if keep and #shown < 12 then
				table.insert(shown, s)
			end
		end

		local rows = {}
		for _, s in shown do
			table.insert(
				rows,
				ui.card({
					ui.text(s.title, { size = "small" }),
					ui.text(`{s.provider} · {state(s)}`, {
						size = "caption",
						color = if s.needs_approval then "warning" elseif s.busy then "accent" else "muted",
					}),
					ui.button("Open", function()
						host.open_session(s.id)
					end),
				}, { gap = 6 })
			)
		end
		if #rows == 0 then
			table.insert(rows, ui.text("Nothing here.", { color = "muted" }))
		end

		return ui.column({
			ui.grid({
				ui.stat("Working", working, { color = "accent" }),
				ui.stat("Waiting", waiting, { color = if waiting > 0 then "warning" else "ink" }),
				ui.stat("Unread", unread),
			}, { columns = 3, gap = 8 }),
			ui.tabs({ "All", "Working", "Needs you" }, filter, function(i)
				filter = i
			end),
			ui.column(rows, { gap = 8 }),
			ui.button("New chat", function()
				host.new_chat()
			end),
		}, { gap = 12 })
	end,

	on = {
		-- Re-render when a turn ends so counts stay fresh between snapshots.
		turn_finished = function(_event, _app) end,
	},
}
