-- 盐城盈海测试平台北向：从数据中心读取 4 台英博大机 PCS（pcs1..pcs4）与测控柜，经 MQTT 上送，并接收遥调/遥控下发。
--
-- PCS 点号直接沿用 config/pcs/PCS_英博_大机.xlsx 的序号，不做重映射：
--   序号 1..57      遥测   -> /pds/pcs/{0..3}/yc
--   序号 58..71     遥信   -> /pds/pcs/{0..3}/yx（附通信状态 65535）
--   序号 2000..2029 遥调回读 -> /pds/pcs/{0..3}/yt
--   下发 /asw/pcs/{0..3}/setyt -> dc.dispatch(pcsN, 序号, 值)
--   设备 pcs1..pcs4 对应 pcs 实例 0..3
-- 测控柜点号沿用 config/测控/测控柜_盐城盈海.xlsx 的 id：
--   1..93 遥测 -> /pds/crd/0/yc；1000..1079 遥信 -> /pds/crd/0/yx；2000..2003 遥控，仅下发不回读
--   下发 /asw/crd/0/setyk -> dc.dispatch(crd, id, 值)

MOD = {
    name = "盐城盈海测试平台北向",
    description = "4台英博大机PCS与测控柜点位MQTT上送与下发",
}

local M = {}
M.MOD = MOD
M.HOST = "127.0.0.1"
M.PORT = 1883
M.USE_GENERIC_ROUTES = true

-- dev 必须与 config.json 中 devices 的设备ID一致
local PCS_COUNT = 4
local YC_FIRST, YC_LAST = 1, 57
local YX_FIRST, YX_LAST = 58, 71
local YT_FIRST, YT_LAST = 2000, 2029

local CRD_DEV = "crd"
local CRD_TOPIC_YC = "/pds/crd/0/yc"
local CRD_TOPIC_YX = "/pds/crd/0/yx"
local CRD_TOPIC_SET_YK = "/asw/crd/0/setyk"
local CRD_YC_FIRST, CRD_YC_LAST = 1, 93
local CRD_YX_FIRST, CRD_YX_LAST = 1000, 1079
local CRD_YK_FIRST, CRD_YK_LAST = 2000, 2003

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

local function comm_status_point(raw)
    -- 采集引擎已在 65535 点维护通信故障状态，优先原样透传，避免离线后的缓存点被误判为在线。
    for _, point in ipairs(raw or {}) do
        if point.id == COMM_STATUS_ID and point.value ~= nil then
            return { id = COMM_STATUS_ID, value = tonumber(point.value) == 0 and 0 or 1 }
        end
    end
    -- 兼容没有内置通信点的旧设备：有有效数据视为在线，空帧视为离线。
    for _, point in ipairs(raw or {}) do
        if point.value ~= nil then return { id = COMM_STATUS_ID, value = 0 } end
    end
    return { id = COMM_STATUS_ID, value = 1 }
end

function M.collect_routes()
    local routes = {}

    -- PCS：设备 pcs1..pcs4 对应 pcs 实例 0..3
    for n = 1, PCS_COUNT do
        local raw = dc.read_all("pcs" .. n) or {}
        local topic = "/pds/pcs/" .. (n - 1)

        local yc = pick_points(raw, YC_FIRST, YC_LAST)
        if #yc > 0 then
            table.insert(routes, { topic = topic .. "/yc", points = yc })
        end

        -- PCS 遥信：58..71 与后端 PcsMwYX.java 保持一致，并附带通信状态字。
        local yx = pick_points(raw, YX_FIRST, YX_LAST)
        table.insert(yx, comm_status_point(raw))
        -- 现场后端的 MQTT JSON 解析器不接受 _faultsVersion/_faults 扩展字段，
        -- 因此只上送原始遥信点和 65535 通信诊断点。
        table.insert(routes, { topic = topic .. "/yx", points = yx })

        local yt = pick_points(raw, YT_FIRST, YT_LAST)
        if #yt > 0 then
            table.insert(routes, { topic = topic .. "/yt", points = yt })
        end
    end

    local crd_raw = dc.read_all(CRD_DEV) or {}
    local crd_yc = pick_points(crd_raw, CRD_YC_FIRST, CRD_YC_LAST)
    if #crd_yc > 0 then
        table.insert(routes, { topic = CRD_TOPIC_YC, points = crd_yc })
    end
    local crd_yx = pick_points(crd_raw, CRD_YX_FIRST, CRD_YX_LAST)
    table.insert(crd_yx, comm_status_point(crd_raw))
    table.insert(routes, { topic = CRD_TOPIC_YX, points = crd_yx })

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
    for n = 1, PCS_COUNT do
        local dev = "pcs" .. n
        conn:subscribe("/asw/pcs/" .. (n - 1) .. "/setyt", function(_, payload)
            handle_set_yt(dev, YT_FIRST, YT_LAST, payload)
        end)
    end
    conn:subscribe(CRD_TOPIC_SET_YK, function(_, payload)
        handle_set_yt(CRD_DEV, CRD_YK_FIRST, CRD_YK_LAST, payload)
    end)
end

return M
