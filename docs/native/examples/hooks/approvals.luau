--!strict
-- An approval policy. Copy to <data>/hooks/approvals.luau to use it.
-- Return "allow", "session" (allow for the rest of the session), "deny",
-- or nil to ask as usual. When several hooks answer, any "deny" wins.

local function has(text: string, needle: string): boolean
	return string.find(string.lower(text), needle, 1, true) ~= nil
end

return {
	approval = function(request)
		local text = `{request.title}\n{request.detail}`
		-- Never let an agent force-push or wipe directories.
		if has(text, "git push --force") or has(text, "git push -f") or has(text, "rm -rf /") then
			return "deny"
		end
		-- Reading and searching are always fine. `tool` is set for providers
		-- that name the tool, such as Claude Code ("Allow Read?").
		local reads = { Read = true, Glob = true, Grep = true, LS = true, WebSearch = true }
		if request.tool and reads[request.tool] then
			return "allow"
		end
		-- Everything else: ask.
		return nil
	end,

	turn_finished = function(event)
		if not event.cancelled then
			host.notify(`{event.title} finished`)
		end
	end,
}
