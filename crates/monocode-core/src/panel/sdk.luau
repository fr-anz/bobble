--!strict
-- The panel SDK. It runs before sandboxing and returns the host controller;
-- programs only see the frozen `ui` and `host` tables it installs.
local handlers: { [string]: { fn: (any) -> (), accepts: (any) -> boolean, disabled: boolean } } = {}
local handlerCount = 0
local generation = 0
local actions: { any } = {}
local program: any = nil

local function check(condition: any, message: string)
	if not condition then
		error(message, 3)
	end
end

local function options(value: any, allowed: { string }): { [string]: any }
	if value == nil then
		return {}
	end
	check(type(value) == "table", "Options must be a table")
	for key in value do
		check(type(key) == "string" and table.find(allowed, key) ~= nil, `Unsupported style or option: {tostring(key)}`)
	end
	return value
end

local function text(value: any): string
	check(type(value) == "string" or type(value) == "number", "Text must be a string or number")
	return tostring(value)
end

local function optionalText(value: any): string?
	if value == nil then
		return nil
	end
	return text(value)
end

local function list(value: any, message: string): { any }
	check(type(value) == "table", message)
	return value
end

local function register(fn: any, accepts: (any) -> boolean, disabled: boolean?): string
	check(type(fn) == "function", "Interactive controls require a function handler")
	check(handlerCount < 512, "Too many panel handlers")
	local id = `{generation}:{handlerCount}`
	handlerCount += 1
	handlers[id] = { fn = fn, accepts = accepts, disabled = disabled == true }
	return id
end

local function layout(kind: string, nodes: any, o: any)
	o = options(o, if kind == "grid" then { "gap", "columns" } else { "gap" })
	list(nodes, "Layout children must be a list")
	check(#nodes <= 512, "Too many layout children")
	local node: { [string]: any } = { kind = kind, children = nodes, gap = o.gap or 12 }
	if kind == "grid" then
		node.columns = o.columns or 2
	end
	return node
end

local function color(o: { [string]: any }): string
	return o.color or "accent"
end

local function texts(values: any): { string }
	local out = {}
	for i, v in values or {} do
		out[i] = text(v)
	end
	return out
end

local function isInteger(v: any): boolean
	return type(v) == "number" and v == math.floor(v)
end

local ui = {}

function ui.column(nodes, o)
	return layout("column", nodes, o)
end
function ui.row(nodes, o)
	return layout("row", nodes, o)
end
function ui.grid(nodes, o)
	return layout("grid", nodes, o)
end
function ui.card(nodes, o)
	return layout("card", nodes, o)
end
function ui.text(value, o)
	o = options(o, { "size", "color" })
	return { kind = "text", text = text(value), size = o.size or "body", color = o.color or "ink" }
end
function ui.stat(label, value, o)
	o = options(o, { "detail", "color", "delta", "trend", "better" })
	check(o.better == nil or o.better == "higher" or o.better == "lower", "Stat better must be 'higher' or 'lower'")
	return {
		kind = "stat",
		label = text(label),
		value = text(value),
		detail = optionalText(o.detail),
		color = o.color or "ink",
		delta = optionalText(o.delta),
		trend = o.trend or {},
		lower_is_better = o.better == "lower",
	}
end
function ui.gauge(label, value, o)
	o = options(o, { "min", "max", "unit", "color" })
	return {
		kind = "gauge",
		label = text(label),
		value = value,
		min = o.min or 0,
		max = o.max or 100,
		unit = optionalText(o.unit),
		color = color(o),
	}
end
function ui.donut(slices, o)
	o = options(o, { "label", "value" })
	list(slices, "Donut slices must be a list")
	local out = {}
	for i, s in slices do
		options(s, { "label", "value", "color" })
		out[i] = { label = text(s.label), value = s.value, color = s.color or `series{(i - 1) % 6 + 1}` }
	end
	return { kind = "donut", label = optionalText(o.label), value = optionalText(o.value), slices = out }
end
function ui.heatmap(values, o)
	o = options(o, { "rows", "columns", "color" })
	list(values, "Heatmap values must be a list of rows")
	return {
		kind = "heatmap",
		values = values,
		rows = texts(o.rows),
		columns = texts(o.columns),
		color = o.color or "positive",
	}
end
function ui.progress(label, value, o)
	o = options(o, { "max", "detail", "color" })
	return {
		kind = "progress",
		label = text(label),
		value = value,
		max = o.max or 100,
		detail = optionalText(o.detail),
		color = color(o),
	}
end
function ui.table(columns, rows)
	list(columns, "Table columns must be a list")
	list(rows, "Table rows must be a list")
	local out = {}
	for i, row in rows do
		out[i] = texts(list(row, "Table rows must be lists"))
	end
	return { kind = "table", columns = texts(columns), rows = out }
end
function ui.chart(chartType, series, o)
	o = options(o, { "labels", "height" })
	list(series, "Chart series must be a list")
	local out = {}
	for i, s in series do
		options(s, { "label", "values", "color" })
		out[i] = { label = text(s.label), values = s.values, color = s.color or `series{(i - 1) % 6 + 1}` }
	end
	return { kind = "chart", chart_type = chartType, labels = texts(o.labels), height = o.height or 240, series = out }
end
function ui.button(label, fn, o)
	o = options(o, { "disabled" })
	local disabled = o.disabled == true
	return {
		kind = "button",
		label = text(label),
		disabled = disabled,
		handler = register(fn, function(v)
			return v == nil
		end, disabled),
	}
end
function ui.toggle(label, value, fn)
	return {
		kind = "toggle",
		label = text(label),
		value = value == true,
		handler = register(fn, function(v)
			return type(v) == "boolean"
		end),
	}
end
function ui.input(label, value, fn, o)
	o = options(o, { "placeholder" })
	return {
		kind = "input",
		label = text(label),
		value = text(value),
		placeholder = optionalText(o.placeholder),
		handler = register(fn, function(v)
			return type(v) == "string" and #v <= 4096
		end),
	}
end
-- Choice indexes are 1-based in Luau and 0-based on the wire.
function ui.select(label, labels, selected, fn)
	list(labels, "Select options must be a list")
	local count = #labels
	return {
		kind = "select",
		label = text(label),
		options = texts(labels),
		selected = (selected or 1) - 1,
		handler = register(function(v)
			fn(v + 1)
		end, function(v)
			return isInteger(v) and v >= 0 and v < count
		end),
	}
end
function ui.tabs(labels, selected, fn)
	list(labels, "Tab labels must be a list")
	local count = #labels
	return {
		kind = "tabs",
		labels = texts(labels),
		selected = (selected or 1) - 1,
		handler = register(function(v)
			fn(v + 1)
		end, function(v)
			return isInteger(v) and v >= 0 and v < count
		end),
	}
end
function ui.slider(label, value, min, max, fn, o)
	o = options(o, { "step" })
	return {
		kind = "slider",
		label = text(label),
		value = value,
		min = min,
		max = max,
		step = o.step or 1,
		handler = register(fn, function(v)
			return type(v) == "number" and v == v and v >= min and v <= max
		end),
	}
end
function ui.canvas(shapes, o)
	o = options(o, { "width", "height" })
	return { kind = "canvas", shapes = list(shapes, "Canvas shapes must be a list"), width = o.width or 640, height = o.height or 320 }
end
function ui.rect(x, y, width, height, o)
	o = options(o, { "color" })
	return { kind = "rect", x = x, y = y, width = width, height = height, color = color(o) }
end
function ui.circle(x, y, radius, o)
	o = options(o, { "color" })
	return { kind = "circle", x = x, y = y, radius = radius, color = color(o) }
end
function ui.line(x1, y1, x2, y2, o)
	o = options(o, { "width", "color" })
	return { kind = "line", x1 = x1, y1 = y1, x2 = x2, y2 = y2, width = o.width or 2, color = color(o) }
end
function ui.polyline(points, o)
	o = options(o, { "width", "color" })
	return { kind = "polyline", points = points, width = o.width or 2, color = color(o) }
end
function ui.label(x, y, value, o)
	o = options(o, { "color" })
	return { kind = "label", x = x, y = y, text = text(value), color = o.color or "ink" }
end
-- A native app view: "explorer", "changes" or "history".
function ui.slot(name, o)
	o = options(o, { "height" })
	check(type(name) == "string", "Slot name must be a string")
	return { kind = "slot", name = name, height = o.height }
end
table.freeze(ui)

-- App actions run after the current call returns, in order.
local host = {}
local function act(kind: string, fields: { [string]: any })
	check(#actions < 32, "Too many actions in one call")
	fields.kind = kind
	table.insert(actions, fields)
end
function host.notify(message)
	act("notify", { message = text(message) })
end
function host.open_file(path)
	act("open_file", { path = text(path) })
end
function host.new_chat(prompt)
	act("new_chat", { prompt = text(prompt or "") })
end
function host.send(session, message)
	check(isInteger(session), "Session id must be an integer")
	act("send", { session = session, message = text(message) })
end
function host.open_session(session)
	check(isInteger(session), "Session id must be an integer")
	act("open_session", { session = session })
end
function host.copy(value)
	act("copy", { text = text(value) })
end
function host.select_tab(name)
	act("select_tab", { tab = text(name) })
end
table.freeze(host)

local function render(app: any): any
	table.clear(handlers)
	handlerCount = 0
	generation += 1
	local node = program.render(app)
	check(type(node) == "table", "render must return a component")
	return node
end

return table.freeze({
	ui = ui,
	host = host,
	init = function(value: any)
		check(type(value) == "table", "A panel must return a table: return { title = 'Name', render = function(app) ... end }")
		check(type(value.render) == "function", "A panel needs a render function")
		check(value.title == nil or type(value.title) == "string", "Panel title must be a string")
		check(value.icon == nil or type(value.icon) == "string", "Panel icon must be a string")
		check(value.refresh == nil or type(value.refresh) == "number", "Panel refresh must be a number of seconds")
		check(value.on == nil or type(value.on) == "table", "Panel `on` must be a table of event handlers")
		program = value
		return { title = value.title, icon = value.icon, refresh = value.refresh }
	end,
	render = render,
	dispatch = function(id: string, value: any, app: any)
		local entry = handlers[id]
		check(entry ~= nil, "This panel control is stale; use the current view")
		check(not entry.disabled, "This panel control is disabled")
		check(entry.accepts(value), "Invalid value for this panel control")
		entry.fn(value)
		return render(app)
	end,
	-- Returns whether the panel handles the event; a handled event re-renders.
	event = function(name: string, payload: any, app: any)
		local handler = program.on and program.on[name]
		if type(handler) ~= "function" then
			return false
		end
		handler(payload, app)
		return true
	end,
	-- Hook files return a table of functions named after app events.
	initHook = function(value: any)
		check(type(value) == "table", "A hook file must return a table: return { approval = function(request) ... end }")
		for key, fn in value do
			check(type(key) == "string" and type(fn) == "function", "Hook tables map event names to functions")
		end
		program = value
		local names = {}
		for key in value do
			table.insert(names, key)
		end
		return names
	end,
	hook = function(name: string, payload: any)
		local handler = program[name]
		if type(handler) ~= "function" then
			return nil
		end
		return handler(payload)
	end,
	takeActions = function()
		local out = actions
		actions = {}
		return out
	end,
})
