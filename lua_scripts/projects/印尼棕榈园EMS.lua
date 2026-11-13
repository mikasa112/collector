-- 印尼棕榈园EMS 北向：从数据中心读取 PCS / BAU / 智能断路器点位，经 MQTT 上送，并接收遥调、遥控下发。
-- 结构与 万润.lua 一致（collect_routes + register_downlinks），点号直接沿用各点表的序号，不做重映射：
--   PCS（英博 125kW，config/pcs/PCS_英博_125.xlsx）
--     遥测 1..179      -> /pds/pcs/0/yc
--     遥信 1000..1015  -> /pds/pcs/0/yx（附通信状态 65535 与故障元数据）
--     遥调回读 2000..2042 -> /pds/pcs/0/yt
--     下发 /asw/pcs/0/setyt -> dc.dispatch(pcs, 2000..2042, 值)
--     下发 /asw/pcs/0/setyk -> dc.dispatch(pcs, 3000..3015, 值)（遥控，仅下发不回读）
--   BAU（高泰昊能，config/bms/BAU_高泰昊能.xlsx）：目前读取的是簇数据（单簇，簇号 0），堆级点位暂不接入，
--   按簇 topic 上送（与万润 BCU 簇一致）：
--     遥测 100..133、2034..2035 -> /pds/bank/0/rack/0/yc
--     状态/告警 2000..2033      -> /pds/bank/0/rack/0/yx（附通信状态与告警位）
--     遥调回读 10000..10123     -> /pds/bank/0/rack/0/yt
--     下发 /asw/bank/0/rack/0/setyt -> dc.dispatch(bau, 10000..10123, 值)
--     未上送：TMS 5000..5010（堆级/热管理，待堆接入时再定 topic）
--   智能断路器（Lua 脚本设备 breaker，lua_scripts/印尼棕榈园区EMS_智能断路器.lua，只读）
--     遥测 1..21 -> /pds/breaker/0/yc；状态字及位域 22..27 -> /pds/breaker/0/yx（附通信状态）

MOD = {
    name = "印尼棕榈园EMS北向",
    description = "PCS/BAU/智能断路器点位MQTT上送与遥调遥控下发",
}

local M = {}
M.MOD = MOD
M.HOST = "127.0.0.1"
M.PORT = 1883
-- 必须为 true：否则 collect_routes 之后引擎还会继续走奥林波斯固定的 CRD/PCS/BANK 上送流程
M.USE_GENERIC_ROUTES = true

-- dev 必须与 config.json 中 devices 的设备ID（或 Lua 脚本 dc.register 的 dev_id）一致
local PCS_DEV = "pcs"
local PCS_TOPIC_YC = "/pds/pcs/0/yc"
local PCS_TOPIC_YX = "/pds/pcs/0/yx"
local PCS_TOPIC_YT = "/pds/pcs/0/yt"
local PCS_TOPIC_SET_YT = "/asw/pcs/0/setyt"
local PCS_TOPIC_SET_YK = "/asw/pcs/0/setyk"
local PCS_YC_FIRST, PCS_YC_LAST = 1, 179
local PCS_YX_FIRST, PCS_YX_LAST = 1000, 1015
local PCS_YT_FIRST, PCS_YT_LAST = 2000, 2042
local PCS_YK_FIRST, PCS_YK_LAST = 3000, 3015

local BAU_DEV = "bau"
local BAU_RACK_ID = 0 -- 簇号（bcuid，从 0 开始）；高泰昊能当前只有 1 个簇
local BAU_RACK = "/pds/bank/0/rack/" .. BAU_RACK_ID
local BAU_TOPIC_YC = BAU_RACK .. "/yc"
local BAU_TOPIC_YX = BAU_RACK .. "/yx"
local BAU_TOPIC_YT = BAU_RACK .. "/yt"
local BAU_TOPIC_SET_YT = "/asw/bank/0/rack/" .. BAU_RACK_ID .. "/setyt"
local BAU_YC_RANGES = { { 100, 133 }, { 2034, 2035 } }
local BAU_YX_RANGES = { { 2000, 2033 } }
local BAU_YT_FIRST, BAU_YT_LAST = 10000, 10123

local BREAKER_DEV = "breaker"
local BREAKER_TOPIC_YC = "/pds/breaker/0/yc"
local BREAKER_TOPIC_YX = "/pds/breaker/0/yx"
local BREAKER_YC_FIRST, BREAKER_YC_LAST = 1, 21
local BREAKER_YX_FIRST, BREAKER_YX_LAST = 22, 27

-- 通信故障字：北向补充遥信点，点号 65535；0=正常在线，1=通信故障/离线
local COMM_STATUS_ID = 65535

local function in_range(id, first, last)
    return type(id) == "number" and id >= first and id <= last
end

-- 取出 [first,last] 范围内且有值的点；无值不上送，避免后端被 null 覆盖最后有效值
local function pick_points(raw, first, last)
    local result = {}
    for _, point in ipairs(raw or {}) do
        if in_range(point.id, first, last) and point.value ~= nil then
            table.insert(result, { id = point.id, value = point.value, faults = point.faults })
        end
    end
    return result
end

-- 取出落在任一 {first,last} 区间内且有值的点
local function pick_ranges(raw, ranges)
    local result = {}
    for _, point in ipairs(raw or {}) do
        if point.value ~= nil then
            for _, r in ipairs(ranges) do
                if in_range(point.id, r[1], r[2]) then
                    table.insert(result, { id = point.id, value = point.value, faults = point.faults })
                    break
                end
            end
        end
    end
    return result
end

-- 只提取 [first,last] 范围内有活动故障位的点的故障元数据，不带 value。
-- PCS 的故障字在遥测号段里，而引擎只在 include_faults=true 的路由上带 _faults；
-- 这里把它们挂到 yx 路由，既不改变 yc 的载荷，也不让告警丢失（value 为空时引擎只会输出 _faults，不会写点值）。
local function pick_faults(raw, first, last)
    local result = {}
    for _, point in ipairs(raw or {}) do
        if in_range(point.id, first, last) and type(point.faults) == "table" and #point.faults > 0 then
            table.insert(result, { id = point.id, faults = point.faults })
        end
    end
    return result
end

local function append_all(dst, src)
    for _, point in ipairs(src) do
        table.insert(dst, point)
    end
end

local function comm_status_point(raw)
    local function build(value)
        return { id = COMM_STATUS_ID, value = value }
    end
    -- 采集引擎已在 65535 点维护通信故障状态，优先原样透传，避免离线后的缓存点被误判为在线。
    for _, point in ipairs(raw or {}) do
        if point.id == COMM_STATUS_ID and point.value ~= nil then
            return build(tonumber(point.value) == 0 and 0 or 1)
        end
    end
    -- 兼容没有内置通信点的设备：有有效数据视为在线，空帧视为离线。
    for _, point in ipairs(raw or {}) do
        if point.value ~= nil then return build(0) end
    end
    return build(1)
end

function M.collect_routes()
    local routes = {}

    -- PCS
    local pcs_raw = dc.read_all(PCS_DEV) or {}
    local pcs_yc = pick_points(pcs_raw, PCS_YC_FIRST, PCS_YC_LAST)
    if #pcs_yc > 0 then
        table.insert(routes, { topic = PCS_TOPIC_YC, points = pcs_yc })
    end
    local pcs_yx = pick_points(pcs_raw, PCS_YX_FIRST, PCS_YX_LAST)
    append_all(pcs_yx, pick_faults(pcs_raw, PCS_YC_FIRST, PCS_YC_LAST))
    table.insert(pcs_yx, comm_status_point(pcs_raw))
    table.insert(routes, { topic = PCS_TOPIC_YX, points = pcs_yx, include_faults = true })
    local pcs_yt = pick_points(pcs_raw, PCS_YT_FIRST, PCS_YT_LAST)
    if #pcs_yt > 0 then
        table.insert(routes, { topic = PCS_TOPIC_YT, points = pcs_yt })
    end

    -- BAU 簇：按点号分段拆成 rack/{n}/yx|yc|yt 三个 topic，yx 附带通信状态字与告警位
    local bau_raw = dc.read_all(BAU_DEV) or {}
    local bau_yx = pick_ranges(bau_raw, BAU_YX_RANGES)
    table.insert(bau_yx, comm_status_point(bau_raw))
    table.insert(routes, { topic = BAU_TOPIC_YX, points = bau_yx, include_faults = true })
    local bau_yc = pick_ranges(bau_raw, BAU_YC_RANGES)
    if #bau_yc > 0 then
        table.insert(routes, { topic = BAU_TOPIC_YC, points = bau_yc })
    end
    local bau_yt = pick_points(bau_raw, BAU_YT_FIRST, BAU_YT_LAST)
    if #bau_yt > 0 then
        table.insert(routes, { topic = BAU_TOPIC_YT, points = bau_yt })
    end

    -- 智能断路器（Lua 脚本设备，只读）
    local breaker_raw = dc.read_all(BREAKER_DEV) or {}
    local breaker_yc = pick_points(breaker_raw, BREAKER_YC_FIRST, BREAKER_YC_LAST)
    if #breaker_yc > 0 then
        table.insert(routes, { topic = BREAKER_TOPIC_YC, points = breaker_yc })
    end
    local breaker_yx = pick_points(breaker_raw, BREAKER_YX_FIRST, BREAKER_YX_LAST)
    table.insert(breaker_yx, comm_status_point(breaker_raw))
    table.insert(routes, { topic = BREAKER_TOPIC_YX, points = breaker_yx })

    return routes
end

local function handle_set_yt(dev, first, last, payload)
    local ok_decode, message = pcall(json.decode, payload)
    if not ok_decode or type(message) ~= "table" then
        log.warn("下发格式错误 dev=" .. dev .. ": " .. tostring(payload))
        return
    end
    for id_text, value in pairs(message) do
        local id = tonumber(id_text)
        if in_range(id, first, last) then
            local ok_dispatch, err = pcall(dc.dispatch, dev, id, value)
            if not ok_dispatch then
                log.warn("下发失败 dev=" .. dev .. " id=" .. id .. ": " .. tostring(err))
            end
        else
            log.warn("下发点号不在允许范围 dev=" .. dev .. " id=" .. tostring(id_text))
        end
    end
end

function M.register_downlinks(conn)
    conn:subscribe(PCS_TOPIC_SET_YT, function(_, payload)
        handle_set_yt(PCS_DEV, PCS_YT_FIRST, PCS_YT_LAST, payload)
    end)
    conn:subscribe(PCS_TOPIC_SET_YK, function(_, payload)
        handle_set_yt(PCS_DEV, PCS_YK_FIRST, PCS_YK_LAST, payload)
    end)
    conn:subscribe(BAU_TOPIC_SET_YT, function(_, payload)
        handle_set_yt(BAU_DEV, BAU_YT_FIRST, BAU_YT_LAST, payload)
    end)
end

return M
