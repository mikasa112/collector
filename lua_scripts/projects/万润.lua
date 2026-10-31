-- 万润项目北向：从数据中心读取英博大机 PCS 点位，经 MQTT 上送，并接收遥调下发。
--
-- 点号直接沿用 config/pcs/PCS_英博_大机.xlsx 的序号，不做重映射：
--   sheet1 序号 1..57    遥测 -> /pds/pcs_mw/yc
--   sheet2 序号 2000..2029 遥调回读 -> /pds/pcs_mw/yt
--   下发 /pds/pcs_mw/setyt -> dc.dispatch(pcs, 序号, 值)
-- 测控柜点号沿用 config/测控/测控柜_万润.xlsx 的 id：
--   1..59 遥测 -> /pds/crd/yc；60..86 遥信 -> /pds/crd/yx；87..88 遥控合分闸，仅下发不回读
--   下发 /pds/crd/setyk -> dc.dispatch(crd, id, 值)

MOD = {
    name = "万润项目北向",
    description = "英博大机PCS点位MQTT上送与遥调下发",
}

local M = {}
M.MOD = MOD
M.HOST = "127.0.0.1"
M.PORT = 1883
M.USE_GENERIC_ROUTES = true

-- dev 必须与 config.json 中 devices 的设备ID一致
local PCS_DEV = "pcs"
local TOPIC_YC = "/pds/pcs_mw/0/yc"
local TOPIC_YT = "/pds/pcs_mw/0/yt"
local TOPIC_SET_YT = "/asw/pcs_mw/0/setyt"

local YC_FIRST, YC_LAST = 1, 57
local YT_FIRST, YT_LAST = 2000, 2029

local CRD_DEV = "crd"
local CRD_TOPIC_YC = "/pds/crd/0/yc"
local CRD_TOPIC_YX = "/pds/crd/0/yx"
local CRD_TOPIC_SET_YK = "/asw/crd/0/setyk"
local CRD_YC_FIRST, CRD_YC_LAST = 1, 59
local CRD_YX_FIRST, CRD_YX_LAST = 60, 86
local CRD_YK_FIRST, CRD_YK_LAST = 87, 88

local function in_range(id, first, last)
    return type(id) == "number" and id >= first and id <= last
end

-- 取出 [first,last] 范围内且有值的点；无值不上送，避免后端被 null 覆盖最后有效值
local function pick_points(raw, first, last)
    local result = {}
    for _, point in ipairs(raw or {}) do
        if in_range(point.id, first, last) and point.value ~= nil then
            table.insert(result, { id = point.id, value = point.value })
        end
    end
    return result
end

function M.collect_routes()
    local raw = dc.read_all(PCS_DEV) or {}
    local routes = {}

    local yc = pick_points(raw, YC_FIRST, YC_LAST)
    if #yc > 0 then
        table.insert(routes, { topic = TOPIC_YC, points = yc })
    end

    local yt = pick_points(raw, YT_FIRST, YT_LAST)
    if #yt > 0 then
        table.insert(routes, { topic = TOPIC_YT, points = yt })
    end

    local crd_raw = dc.read_all(CRD_DEV) or {}
    local crd_yc = pick_points(crd_raw, CRD_YC_FIRST, CRD_YC_LAST)
    if #crd_yc > 0 then
        table.insert(routes, { topic = CRD_TOPIC_YC, points = crd_yc })
    end
    local crd_yx = pick_points(crd_raw, CRD_YX_FIRST, CRD_YX_LAST)
    if #crd_yx > 0 then
        table.insert(routes, { topic = CRD_TOPIC_YX, points = crd_yx })
    end

    return routes
end

local function handle_set_yt(dev, first, last, payload)
    local ok_decode, message = pcall(json.decode, payload)
    if not ok_decode or type(message) ~= "table" then
        log.warn("遥调下发格式错误 dev=" .. dev .. ": " .. tostring(payload))
        return
    end
    for id_text, value in pairs(message) do
        local id = tonumber(id_text)
        if in_range(id, first, last) then
            local ok_dispatch, err = pcall(dc.dispatch, dev, id, value)
            if not ok_dispatch then
                log.warn("遥调下发失败 dev=" .. dev .. " id=" .. id .. ": " .. tostring(err))
            end
        else
            log.warn("遥调点号不在允许范围 dev=" .. dev .. " id=" .. tostring(id_text))
        end
    end
end

function M.register_downlinks(conn)
    conn:subscribe(TOPIC_SET_YT, function(_, payload)
        handle_set_yt(PCS_DEV, YT_FIRST, YT_LAST, payload)
    end)
    conn:subscribe(CRD_TOPIC_SET_YK, function(_, payload)
        handle_set_yt(CRD_DEV, CRD_YK_FIRST, CRD_YK_LAST, payload)
    end)
end

return M
