-- 万润 PCS 南向点表 -> MQTT 物理模型契约测试。
-- 运行：在 collector 目录执行 `lua tests/wanrun_pcs_contract_test.lua`

package.path = "lua_scripts/?.lua;lua_scripts/?/init.lua;" .. package.path
package.loaded["projects.527AMC电表_万润"] = true
log = { error = function() end, warn = function() end, info = function() end }

local project = require("projects.万润")
local test = assert(project._test)

local function equal(actual, expected, message)
    if actual ~= expected then
        error((message or "assertion failed") .. ": expected=" .. tostring(expected) .. ", actual=" .. tostring(actual))
    end
end

local function by_id(points)
    local result = {}
    for _, point in ipairs(points) do result[point.id] = point.value end
    return result
end

equal(test.detect_pcs_model({ { id = 1, key = "pcsPortAPhVolt", value = 400 } }), "PCS_KW", "125kW protocol detection")
equal(test.detect_pcs_model({ { id = 1, key = "workStatus", value = 2 } }), "PCS_MW", "MW protocol detection")

local kw_yc, kw_yx, kw_yt, kw_downlink, kw_comm = test.pcs_profile("PCS_KW")
local kw_values = by_id(test.map_points({
    { id = 1, value = 380.1 }, { id = 35, value = 12 }, { id = 59, value = 760.0 },
    { id = 133, value = 1 }, { id = 139, value = 10.5 }, { id = 155, value = 73.0 },
}, kw_yc))
equal(kw_values[1], 380.1, "kW base telemetry")
equal(kw_values[34], 12, "kW clock remap")
equal(kw_values[40], 760.0, "kW DCDC remap")
equal(kw_values[46], 1, "kW derating remap")
equal(kw_values[52], 10.5, "kW branch current remap")
equal(kw_values[68], 73.0, "kW chip temperature remap")
equal(by_id(test.map_points({ { id = 1000, value = 1 }, { id = 156, value = 7 }, { id = 164, value = 9 } }, kw_yx))[1014], 7, "kW fault word remap")
equal(by_id(test.map_points({ { id = 2028, value = 58 }, { id = 2035, value = 0.98 } }, kw_yt))[2024], 58, "kW setting readback remap")
equal(kw_downlink[2004], 2003, "kW active power downlink")
equal(kw_downlink[2036], 3000, "kW reset coil downlink")
equal(kw_comm, 1001, "kW communication point")

local mw_yc, mw_yx, mw_yt, mw_downlink, mw_comm = test.pcs_profile("PCS_MW")
equal(by_id(test.map_points({ { id = 18, value = 1250.0 } }, mw_yc))[18], 1250.0, "MW active power passthrough")
equal(by_id(test.map_points({ { id = 62, value = 3 }, { id = 69, value = 4 } }, mw_yx))[62], 3, "MW fault passthrough")
equal(by_id(test.map_points({ { id = 2002, value = 800.0 } }, mw_yt))[2002], 800.0, "MW setting passthrough")
equal(mw_downlink, nil, "MW downlink is direct")
equal(mw_comm, 0xFFFF, "MW communication point")

print("wanrun PCS contract tests passed")
