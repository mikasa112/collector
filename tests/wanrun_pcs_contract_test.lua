-- 万润生产北向 Topic 与点号分流契约测试。
-- 运行：在 collector 目录执行 `lua tests/wanrun_pcs_contract_test.lua`

package.path = "lua_scripts/?.lua;lua_scripts/?/init.lua;" .. package.path
log = { error = function() end, warn = function() end, info = function() end }

local frames = {
    pcs = { { id = 1, value = 16 }, { id = 57, value = 0 }, { id = 58, value = 1, faults = { { bit = 0, zh = "与主控通信故障", en = "Communication failure with main control", level = 2 } } }, { id = 71, value = 2 }, { id = 2000, value = 2 }, { id = 2029, value = 0 } },
    bau = { { id = 1, value = 1, faults = { { bit = 0, zh = "BMS内部通讯故障", en = "BMS internal communication fault", level = 2 } } }, { id = 12, value = 72 }, { id = 27, value = { 0, 0 } }, { id = 50, value = 1372.4 }, { id = 2171, value = 1 } },
    crd = { { id = 1, value = 235.9 }, { id = 59, value = 0 }, { id = 60, value = 0 }, { id = 86, value = 0 } },
    amc_meter_1 = { { id = 1, value = 233.9 }, { id = 65535, value = 0 } },
    amc_meter_2 = { { id = 1, value = 234.4 }, { id = 65535, value = 0 } },
    pd606_10kv_meter_1 = { { id = 1, value = 10173.4 }, { id = 65535, value = 1 } },
    pd606_10kv_meter_2 = { { id = 1, value = 10167.4 }, { id = 65535, value = 0 } },
}
for n = 1, 12 do
    frames["bcu" .. n] = { { id = 3, value = 1, faults = { { bit = 0, zh = "总压过压", en = "Total voltage overvoltage", level = 3 } } }, { id = 9, value = 3 }, { id = 17, value = 1, faults = { { bit = 0, zh = "从控告警", en = "Slave alarm", level = 2 } } }, { id = 87, value = 1996 }, { id = 200, value = { 3.29, 3.30 } }, { id = 201, value = { 21.0 } }, { id = 2207, value = 26 } }
end
dc = { read_all = function(dev) return frames[dev] or {} end }

local project = require("projects.万润")
local routes = project.collect_routes()
local by_topic, route_by_topic = {}, {}
for _, route in ipairs(routes) do
    by_topic[route.topic] = route.points
    route_by_topic[route.topic] = route
end

local function exists(topic) assert(by_topic[topic], "missing route: " .. topic) end
exists("/pds/pcs/0/yc")
exists("/pds/pcs/0/yx")
exists("/pds/pcs/0/yt")
local pcs_yx = {}
for _, point in ipairs(by_topic["/pds/pcs/0/yx"]) do pcs_yx[point.id] = point.value end
assert(pcs_yx[58] == 1, "missing PCSMW YX 58")
assert(pcs_yx[71] == 2, "missing PCSMW YX 71")
assert(pcs_yx[65535] == 0, "online PCSMW must report communication fault=0")
assert(route_by_topic["/pds/pcs/0/yx"].include_faults == true, "PCSMW YX must publish point-table fault metadata")
exists("/pds/bank/0/yc")
exists("/pds/bank/0/yx")
exists("/pds/bank/0/yt")
exists("/pds/crd/0/yc")
exists("/pds/crd/0/yx")
for rack = 0, 11 do
    exists("/pds/bank/0/rack/" .. rack .. "/yc")
    exists("/pds/bank/0/rack/" .. rack .. "/yx")
    exists("/pds/bank/0/rack/" .. rack .. "/yt")
    exists("/pds/bank/0/cell/" .. rack .. "/yc")
end
local first_bcu_yx = by_topic["/pds/bank/0/rack/0/yx"]
local has_point_17, has_critical_fault = false, false
for _, point in ipairs(first_bcu_yx) do
    if point.id == 17 then has_point_17 = true end
    if point.id == 3 and point.faults and point.faults[1] and point.faults[1].level == 3 then
        has_critical_fault = true
    end
end
assert(has_point_17, "BCU alarm point 17 must be routed as YX")
assert(has_critical_fault, "BCU fault metadata must preserve level from point table")

for index = 0, 3 do
    exists("/pds/meter/" .. index .. "/yc")
    exists("/pds/meter/" .. index .. "/yx")
end
local meter2_yx = by_topic["/pds/meter/2/yx"]
assert(meter2_yx[1].value == 1, "10kV meter communication fault must be active")
assert(meter2_yx[1].faults and meter2_yx[1].faults[1].level == 3,
    "10kV meter communication fault must be level 3")
assert(route_by_topic["/pds/meter/2/yx"].include_faults == true,
    "10kV meter YX must publish fault metadata")
local meter3_yx = by_topic["/pds/meter/3/yx"]
assert(meter3_yx[1].value == 0 and meter3_yx[1].faults == nil,
    "online 10kV meter must not publish an active communication fault")
assert(route_by_topic["/pds/meter/0/yx"].include_faults == false,
    "400V meter communication status must keep its existing non-alarm behavior")

frames.pcs = { { id = 65535, value = 1 } }
local offline_routes = project.collect_routes()
for _, route in ipairs(offline_routes) do
    if route.topic == "/pds/pcs/0/yx" then
        local offline_yx = {}
        for _, point in ipairs(route.points) do offline_yx[point.id] = point.value end
        assert(offline_yx[65535] == 1, "offline PCSMW must report communication fault=1")
    end
end

print("wanrun production route contract tests passed")
