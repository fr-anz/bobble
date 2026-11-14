--!strict
-- Board: the project board's lanes at a glance, refreshed every 5 seconds.

local lanes = {
	{ id = "backlog", label = "Backlog", color = "muted" },
	{ id = "in_progress", label = "In progress", color = "accent" },
	{ id = "needs_review", label = "Needs review", color = "warning" },
}

return {
	title = "Board",
	icon = "dashboard",
	refresh = 5,

	render = function(app)
		local counts = {}
		local slices = {}
		for i, lane in lanes do
			local n = 0
			for _, card in app.board do
				if card.lane == lane.id then
					n += 1
				end
			end
			counts[lane.id] = n
			if n > 0 then
				table.insert(slices, { label = lane.label, value = n, color = `series{i}` })
			end
		end

		local sections = {}
		for _, lane in lanes do
			local cards = {}
			for _, card in app.board do
				if card.lane == lane.id and #cards < 8 then
					table.insert(cards, ui.text(`#{card.number} {card.title}`, { size = "small" }))
				end
			end
			if #cards > 0 then
				table.insert(sections, ui.card({
					ui.text(`{lane.label} · {counts[lane.id]}`, { size = "caption", color = lane.color }),
					ui.column(cards, { gap = 4 }),
				}, { gap = 8 }))
			end
		end

		if #slices == 0 then
			return ui.column({
				ui.text("The board is empty.", { color = "muted" }),
				ui.button("Ask for a plan", function()
					host.new_chat("Break this project's next milestone into board cards.")
				end),
			})
		end
		return ui.column({
			ui.donut(slices, { label = "Cards", value = tostring(#app.board) }),
			ui.column(sections, { gap = 8 }),
		}, { gap = 12 })
	end,
}
