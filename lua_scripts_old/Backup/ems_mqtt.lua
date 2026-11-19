MOD = {
    name  = "EMS北向MQTT规则",
    description = "EMS北向MQTT规则",
}

-- local HOST = "192.168.4.120"
local HOST = "192.168.4.120"
local PORT = 1883
local TOPIC_SET_YT = "/asw/emu/2/setyt"
local TOPIC_SET_YK = "/asw/emu/2/setyk"
local TOPIC_YC = "/pds/emu/2/yc"
local TOPIC_YX = "/pds/emu/2/yx"
local TOPIC_YT = "/pds/emu/2/yt"
local TOPIC_YK = "/pds/emu/2/yk"

local utils = require("_utils")

---@type MqttConn
local conn = nil

-- 遥调/遥控的“虚拟id -> {dev, id}”反查表，由 collect_points() 每轮刷新，
-- 用于把下行指令里的聚合虚拟id映射回原始设备点位
local yt_map = {}
local yk_map = {}

--- 按反查表把下行指令派发到原始设备点位
---@param map   table<integer, {dev: string, id: integer}>
---@param label string 日志前缀，如 "遥调"/"遥控"
---@param item  {id: integer, value: number}
local function dispatch_by_map(map, label, item)
    local target = map[item.id]
    if not target then
        log.warn("EMS MQTT " .. label .. "下发失败: 未知点位id=" .. tostring(item.id))
        return
    end
    local ok, err = pcall(dc.dispatch, target.dev, target.id, item.value)
    if not ok then
        log.warn("EMS MQTT " .. label .. "下发失败 dev=" .. target.dev .. " id=" .. tostring(target.id) .. ": " .. tostring(err))
    end
end

--- 通用下行消息解析：消息格式为 {"虚拟id": 值, ...}，校验格式后按 map 逐条派发
---@param map     table<integer, {dev: string, id: integer}>
---@param label   string
---@param payload string
local function handle_set(map, label, payload)
    local ok, msg = pcall(json.decode, payload)
    if not ok or type(msg) ~= "table" then
        log.warn("EMS MQTT " .. label .. "下行消息格式错误: " .. tostring(payload))
        return
    end
    for id_str, value in pairs(msg) do
        local id = tonumber(id_str)
        if id == nil then
            log.warn("EMS MQTT " .. label .. "下行消息id非法: " .. tostring(id_str))
        else
            dispatch_by_map(map, label, { id = id, value = value })
        end
    end
end

local function handle_set_yt(_topic, payload)
    handle_set(yt_map, "遥调", payload)
end

local function handle_set_yk(_topic, payload)
    handle_set(yk_map, "遥控", payload)
end

local function connect_mqtt()
    while true do
        local c, err = mqtt.connect({
            host = HOST,
            port = PORT,
            username = "root",
            password = "inpower",
            max_packet_size = 1024 * 256
        })
        if c then
            conn = c
            log.info("EMS MQTT连接成功:" .. HOST .. ":" .. PORT)
            local ok, sub_err = pcall(function()
                conn:subscribe(TOPIC_SET_YT, handle_set_yt)
                conn:subscribe(TOPIC_SET_YK, handle_set_yk)
            end)
            if not ok then
                log.warn("EMS MQTT 订阅下行 topic 失败:" .. tostring(sub_err))
            end
            return
        end
        log.warn("EMS MQTT 连接失败: " .. tostring(err) .. "，5秒后重试")
        wait(5000)
    end
end

local function collect_points()
    local pcs_data = dc.read_all("pcs")
    local pcs_yc_data = utils.filter(pcs_data, function(value, index)
        return value.id ~= nil and value.id >= 1 and value.id <= 179
    end)
    local pcs_yx_data = utils.filter(pcs_data, function(value, index)
        return value.id ~= nil and value.id >= 1000 and value.id <= 1015
    end)
    local pcs_yt_data = utils.tag_dev(utils.filter(pcs_data, function(value, index)
        return value.id ~= nil and value.id >= 2000 and value.id <= 2042
    end), "pcs")
    local pcs_yk_data = utils.tag_dev(utils.filter(pcs_data, function(value, index)
        return value.id ~= nil and value.id >= 3000 and value.id <= 3007
    end), "pcs")
    local bcu_data = dc.read_all("bcu")
    local bcu_yc_data = utils.filter(bcu_data, function(value, index)
        return value.id ~= nil and value.id >= 4 and value.id <= 53
    end)
    local bcu_yx_data = utils.filter(bcu_data, function(value, index)
        return value.id ~= nil and value.id >= 100 and value.id <= 122
    end)
    local tms_data = dc.read_all("tms")
    local tmc_yc_data = utils.filter(tms_data, function(value, index)
        return value.id ~= nil and value.id >= 1  and value.id <= 19
    end)
    local tmc_yx_data = utils.filter(tms_data, function(value, index)
        return value.id ~= nil and value.id >= 20  and value.id <= 24
    end)
    local tmc_yt_data = utils.tag_dev(utils.filter(tms_data, function(value, index)
        return value.id ~= nil and value.id >= 2000  and value.id <= 2012
    end), "tms")
    local dehum_data = dc.read_all("dehum")
    local dehum_yc_data = utils.filter(dehum_data, function(value, index)
        return value.id ~= nil and value.id >= 1 and value.id <= 6
    end)
    local file_data = dc.read_all("fire")
    local file_yc_data = utils.filter(file_data, function(value, index)
        return value.id ~= nil and value.id >= 1 and value.id <= 5
    end)
    local emu_data = dc.read_all("emu")
    local emu_yc_data = utils.filter(emu_data, function(value, index)
        return value.id ~= nil and value.id >= 1 and value.id <= 3
    end)
    local emu_yt_data = utils.tag_dev(utils.filter(emu_data, function(value, index)
        return value.id ~= nil and value.id >= 4 and value.id <= 10
    end), "emu")
    local dido = dc.read_all("dido")
    local dido_yc = utils.filter(dido, function(value, index)
        return value.id ~= nil and value.id >= 9 and value.id <= 16
    end)

    -- 按类别聚合所有设备的数据，id 在合并时从各类别的基准值开始累加重新编号，
    -- 避免各设备原始 id 冲突：yc 从 1 开始，yx 从 1000 开始，yt 从 2000 开始，yk 从 3000 开始
    local yc_data = utils.merge_with_id({
        pcs_yc_data, bcu_yc_data, tmc_yc_data, dehum_yc_data, file_yc_data, emu_yc_data, dido_yc,
    }, 1)
    local yx_data = utils.merge_with_id({ pcs_yx_data, bcu_yx_data, tmc_yx_data }, 1000)
    local yt_data = utils.merge_with_id({ pcs_yt_data, tmc_yt_data, emu_yt_data }, 2000)
    local yk_data = utils.merge_with_id({ pcs_yk_data }, 3000)

    -- 刷新遥调/遥控虚拟id反查表，供下行 handle_set_yt/handle_set_yk 使用
    yt_map = utils.build_map(yt_data)
    yk_map = utils.build_map(yk_data)

    return yc_data, yx_data, yt_data, yk_data
end

--- 将数据点裁剪为上送所需的 {"id": 值, ...} 结构。
--- key 显式转成字符串，避免 yc 这类 id 从1连续递增的数据被 json 编码成数组。
---@param points DataPoint[]
---@return table<string, number>
local function to_payload(points)
    local result = {}
    for _, p in ipairs(points) do
        result[tostring(p.id)] = p.value
    end
    return result
end

--- 发布一组数据点到指定 topic，失败时记录日志
---@param topic  string
---@param points DataPoint[]
local function publish(topic, points)
    local ok, err = pcall(function()
        conn:publish(topic, to_payload(points))
    end)
    if not ok then
        log.warn("EMS MQTT 上送失败 topic=" .. topic .. ": " .. tostring(err))
    end
end

task.spawn(function ()
    connect_mqtt()
end)

timer.every(2000, function()
    if not conn then
        log.warn("EMS MQTT 未连接，跳过本次上送")
        return
    end
    local yc_data, yx_data, yt_data, yk_data = collect_points()
    publish(TOPIC_YC, yc_data)
    publish(TOPIC_YX, yx_data)
    publish(TOPIC_YT, yt_data)
    publish(TOPIC_YK, yk_data)
end)
