local modulePath = assert(arg[1], "generated Hyprland module path is required")
local deviceCalls = {}
local commands = {}
local subscriptions = {}
local windowRules = {}

hl = {
  device = function(spec)
    table.insert(deviceCalls, spec)
  end,
  exec_cmd = function(command)
    table.insert(commands, command)
  end,
  on = function(event, callback)
    subscriptions[event] = callback
  end,
  window_rule = function(rule)
    table.insert(windowRules, rule)
  end,
  timer = function()
    return {
      set_enabled = function() end,
    }
  end,
}

assert(loadfile(modulePath))()

-- Either name can be the touchscreen, so each edge disables both.
local function assertDisabled(first, label)
  for offset, name in ipairs({ "wch.cn-touchscreen", "wch.cn-touchscreen-1" }) do
    local call = deviceCalls[first + offset - 1]
    assert(call ~= nil, label .. " did not configure the touchscreen")
    assert(call.name == name, label .. " targeted the wrong device")
    assert(call.enabled == false, label .. " did not disable the touchscreen")
    assert(call.output == nil, label .. " retained a stale output mapping")
  end
end

assertDisabled(1, "initial reconciliation")
assert(#commands == 1, "initial reconciliation did not request the lifecycle service")
assert(commands[1]:match(" start xeneon%-edge%-reconcile%.service$"),
  "initial reconciliation did not start the lifecycle service")

assert(subscriptions["monitor.added"] ~= nil, "monitor.added callback is missing")
subscriptions["monitor.added"]()
assertDisabled(3, "monitor add")
assert(#commands == 2, "monitor add requested duplicate lifecycle retries")
assert(commands[2]:match(" restart xeneon%-edge%-reconcile%.service$"),
  "monitor add did not supersede an in-flight reconciliation")

assert(subscriptions["monitor.removed"] ~= nil, "monitor.removed callback is missing")
subscriptions["monitor.removed"]()
assertDisabled(5, "monitor removal")
assert(#commands == 3, "monitor removal requested duplicate lifecycle retries")
assert(commands[3]:match(" restart xeneon%-edge%-reconcile%.service$"),
  "monitor removal did not supersede an in-flight reconciliation")

assert(#windowRules == 1, "the game window rule is missing")
assert(windowRules[1].match.initial_title == "^xeneon-edge-game$",
  "the game window rule matches the wrong window")
assert(windowRules[1].float == true and windowRules[1].no_focus == true,
  "the game window must float without taking focus")
assert(not windowRules[1].monitor:match("@"), "the game window rule kept a placeholder")
