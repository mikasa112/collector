MOD = {
    name  = "奥林波斯测试平台项目北向",
    description = "EMS北向MQTT规则",
}

local HOST = '127.0.0.1'
local PORT = 1883

local TOPIC_SET_PCS1_YT = "/asw/pcs/0/setyt"
local TOPIC_SET_PCS2_YT = "/asw/pcs/1/setyt"
local TOPIC_SET_BANK1_YT = "/asw/bank/0/setyt"
local TOPIC_SET_BANK2_YT = "/asw/bank/1/setyt"

local utils = require("_utils")

---@type MqttConn
local conn = nil

-- bank 的 yt 上送时已重新编号为从 1 开始，下行需按此表把新编号映射回原始 {dev, id} 才能 dc.dispatch。
-- 每次 bank() 上送时同步刷新，供下方 handle_set_bank*_yt 使用。
local bau1_yt_map = {}
local bau2_yt_map = {}


--- 遥调下行消息解析：消息格式为 {"id": 值, ...}，id 为设备原始点位 id，直接透传下发。
---@param dev_id string
---@param payload string
local function handle_set_yt(dev_id, payload)
    local ok, msg = pcall(json.decode, payload)
    if not ok or type(msg) ~= "table" then
        log.warn("MQTT 遥调下行消息格式错误 dev=" .. dev_id .. ": " .. tostring(payload))
        return
    end
    for id_str, value in pairs(msg) do
        local id = tonumber(id_str)
        if id == nil then
            log.warn("MQTT 遥调下行消息id非法 dev=" .. dev_id .. ": " .. tostring(id_str))
        else
            local ok2, err = pcall(dc.dispatch, dev_id, id, value)
            if not ok2 then
                log.warn("MQTT 遥调下发失败 dev=" .. dev_id .. " id=" .. tostring(id) .. ": " .. tostring(err))
            end
        end
    end
end

--- 遥调下行消息解析（映射版）：消息格式为 {"id": 值, ...}，id 为上送时重新编号后的虚拟 id，
--- 需先经 map 查回原始 {dev, id} 再下发。
---@param map table<integer, {dev: string, id: integer}>
---@param payload string
local function handle_set_yt_mapped(map, payload)
    local ok, msg = pcall(json.decode, payload)
    if not ok or type(msg) ~= "table" then
        log.warn("MQTT 遥调下行消息格式错误: " .. tostring(payload))
        return
    end
    for id_str, value in pairs(msg) do
        local id = tonumber(id_str)
        if id == nil then
            log.warn("MQTT 遥调下行消息id非法: " .. tostring(id_str))
        else
            local target = map[id]
            if target == nil then
                log.warn("MQTT 遥调下行id未知: " .. tostring(id))
            else
                local ok2, err = pcall(dc.dispatch, target.dev, target.id, value)
                if not ok2 then
                    log.warn("MQTT 遥调下发失败 dev=" .. target.dev .. " id=" .. tostring(target.id) .. ": " .. tostring(err))
                end
            end
        end
    end
end

local function handle_set_pcs1_yt(_topic, payload)
    handle_set_yt("pcs1", payload)
end

local function handle_set_pcs2_yt(_topic, payload)
    handle_set_yt("pcs2", payload)
end

local function handle_set_bank1_yt(_topic, payload)
    handle_set_yt_mapped(bau1_yt_map, payload)
end

local function handle_set_bank2_yt(_topic, payload)
    handle_set_yt_mapped(bau2_yt_map, payload)
end

local function connect_mqtt()
    while true do
        local c, err = mqtt.connect({
            host = HOST,
            port = PORT,
            -- username = "root",
            -- password = "inpower",
            max_packet_size = 1024 * 256
        })
        if c then
            conn = c
            log.info("EMS MQTT连接成功:" .. HOST .. ":" .. PORT)
            local ok, sub_err = pcall(function()
                conn:subscribe(TOPIC_SET_PCS1_YT, handle_set_pcs1_yt)
                conn:subscribe(TOPIC_SET_PCS2_YT, handle_set_pcs2_yt)
                conn:subscribe(TOPIC_SET_BANK1_YT, handle_set_bank1_yt)
                conn:subscribe(TOPIC_SET_BANK2_YT, handle_set_bank2_yt)
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

---@param topic string
---@param points DataPoint[]
local function publish(topic, points)
    local ok, err = pcall(function()
        conn:publish(topic, to_payload(points))
    end)
    if not ok then
        log.warn("MQTT 上送失败 topic=" .. topic .. ": " .. tostring(err))
    end
end


-- 每台设备由采集引擎自动注入的通讯状态点固定为 id=0xFFFF(65535)，
-- 不在寄存器配置表里，需要单独摘出来归入 yx。
local COMM_STATUS_ID = 0xFFFF

---@param points DataPoint[]
---@return DataPoint[] data 排除通讯状态点后的普通数据
---@return DataPoint[] comm  该设备的通讯状态点（0或1个）
local function split_comm_status(points)
    local data = utils.filter(points, function(p) return p.id ~= COMM_STATUS_ID end)
    local comm = utils.filter(points, function(p) return p.id == COMM_STATUS_ID end)
    return data, comm
end

--- 聚合 meter/thCtrl/trThCtrl/di 四台设备的数据点。
--- 普通数据 id 按此顺序从 1 开始累加重新编号；
--- 每台设备的 65535 通讯诊断点摘出后同样改为按此顺序从 1 开始累加编号，归入 yx。
---@return DataPoint[] yc_data
---@return DataPoint[] yx_data
local function crd()
    local meter = dc.read_all("meter")
    local thCtrl = dc.read_all("thCtrl")
    local trThCtrl = dc.read_all("trThCtrl")
    local di = dc.read_all("di")

    local meter_data, meter_comm = split_comm_status(meter)
    local thCtrl_data, thCtrl_comm = split_comm_status(thCtrl)
    local trThCtrl_data, trThCtrl_comm = split_comm_status(trThCtrl)
    local di_data, di_comm = split_comm_status(di)

    local yc_data = utils.merge_with_id({ meter_data, thCtrl_data, trThCtrl_data, di_data }, 1)
    local yx_data = utils.merge_with_id({ meter_comm, thCtrl_comm, trThCtrl_comm, di_comm }, 1)

    return yc_data, yx_data
end

---@param points DataPoint[]
---@return DataPoint[] yc_data id 在 [1, 57] 区间的遥测点
---@return DataPoint[] yt_data id 在 [2000, 2029] 区间的遥调点
---@return DataPoint[] yx_data id 在 [58, 71] 区间的遥信点
local function split_pcs_yc_yt(points)
    local yc_data = utils.filter(points, function(p)
        return p.id ~= nil and p.id >= 1 and p.id <= 57
    end)
    local yt_data = utils.filter(points, function(p)
        return p.id ~= nil and p.id >= 2000 and p.id <= 2029
    end)
    local yx_data = utils.filter(points, function(p)
        return p.id ~= nil and p.id >= 58 and p.id <= 71
    end)
    return yc_data, yt_data, yx_data
end

--- 分别读取 pcs1/pcs2 的遥测(yc)、遥调(yt)与遥信(yx)数据，各自独立上送，不合并编号。
--- 遥信包含 id[58,71] 区间点位与固定 id=0xFFFF 的通讯诊断点。
---@return DataPoint[] pcs1_yc
---@return DataPoint[] pcs1_yt
---@return DataPoint[] pcs1_yx
---@return DataPoint[] pcs2_yc
---@return DataPoint[] pcs2_yt
---@return DataPoint[] pcs2_yx
local function pcs()
    local pcs1 = dc.read_all("pcs1")
    local pcs2 = dc.read_all("pcs2")

    local pcs1_data, pcs1_comm = split_comm_status(pcs1)
    local pcs2_data, pcs2_comm = split_comm_status(pcs2)

    local pcs1_yc, pcs1_yt, pcs1_yx_range = split_pcs_yc_yt(pcs1_data)
    local pcs2_yc, pcs2_yt, pcs2_yx_range = split_pcs_yc_yt(pcs2_data)

    local pcs1_yx = utils.concat({ pcs1_yx_range, pcs1_comm })
    local pcs2_yx = utils.concat({ pcs2_yx_range, pcs2_comm })

    return pcs1_yc, pcs1_yt, pcs1_yx, pcs2_yc, pcs2_yt, pcs2_yx
end

---@param points DataPoint[]
---@return DataPoint[] yc_data id 在 [20000, 20040] 区间的遥测点
---@return DataPoint[] yx_data id 在 [1000, 1079] 区间的遥信点
---@return DataPoint[] yt_data id 在 [50000, 50015] 区间的遥调点
local function split_bank_yc_yx_yt(points)
    local yc_data = utils.filter(points, function(p)
        return p.id ~= nil and p.id >= 20000 and p.id <= 20040
    end)
    local yx_data = utils.filter(points, function(p)
        return p.id ~= nil and p.id >= 1000 and p.id <= 1079
    end)
    local yt_data = utils.filter(points, function(p)
        return p.id ~= nil and p.id >= 50000 and p.id <= 50015
    end)
    return yc_data, yx_data, yt_data
end

--- 分别读取 bau1/bau2 的遥测(yc)、遥信(yx)与遥调(yt)数据，各自独立上送，不合并编号。
--- yc、yx、yt 的 id 均各自重新编号为从 1 开始的连续序号；
--- yt 重新编号后原始 {dev, id} 记录到 bau1_yt_map/bau2_yt_map，供下行 dc.dispatch 时映射回原始点位。
---@return DataPoint[] bau1_yc
---@return DataPoint[] bau1_yx
---@return DataPoint[] bau1_yt
---@return DataPoint[] bau2_yc
---@return DataPoint[] bau2_yx
---@return DataPoint[] bau2_yt
local function bank()
    local bau1 = dc.read_all("bau1")
    local bau2 = dc.read_all("bau2")

    local bau1_yc, bau1_yx, bau1_yt = split_bank_yc_yx_yt(bau1)
    local bau2_yc, bau2_yx, bau2_yt = split_bank_yc_yx_yt(bau2)

    bau1_yc = utils.merge_with_id({ bau1_yc }, 1)
    bau1_yx = utils.merge_with_id({ bau1_yx }, 1)
    bau2_yc = utils.merge_with_id({ bau2_yc }, 1)
    bau2_yx = utils.merge_with_id({ bau2_yx }, 1)

    bau1_yt = utils.merge_with_id({ utils.tag_dev(bau1_yt, "bau1") }, 1)
    bau2_yt = utils.merge_with_id({ utils.tag_dev(bau2_yt, "bau2") }, 1)
    bau1_yt_map = utils.build_map(bau1_yt)
    bau2_yt_map = utils.build_map(bau2_yt)

    return bau1_yc, bau1_yx, bau1_yt, bau2_yc, bau2_yx, bau2_yt
end

-- 每个 BAU 下辖 12 个簇，簇 N(1-based) 的 id 区间为
-- [BASE_ID + (N-1)*1000, BASE_ID + (N-1)*1000 + SPAN]，例如遥测(yc)簇1 [21000,21046]、
-- 簇2 [22000,22046]，遥信(yx)簇1 [2000,2087]、簇2 [3000,3087]，以此类推直到簇12。
local RACK_COUNT = 12
local RACK_YC_BASE_ID = 21000
local RACK_YC_SPAN = 46
local RACK_YX_BASE_ID = 2000
local RACK_YX_SPAN = 87

---@param points DataPoint[]
---@param base_id integer 簇1 的 id 区间起点
---@param span integer 簇内 id 区间跨度（含端点）
---@return DataPoint[][] racks 按簇序号(0-based，对应 topic 里的簇号)排列的数组，共 RACK_COUNT 组，各簇 id 均重新编号为从 1 开始
local function split_bank_racks(points, base_id, span)
    local racks = {}
    for i = 0, RACK_COUNT - 1 do
        local start_id = base_id + i * 1000
        local end_id = start_id + span
        local rack = utils.filter(points, function(p)
            return p.id ~= nil and p.id >= start_id and p.id <= end_id
        end)
        racks[i + 1] = utils.merge_with_id({ rack }, 1)
    end
    return racks
end

--- 读取某台 BAU 的数据，按簇拆分后发布到 /pds/bank/{bank_index}/rack/{rack_index}/{yc,yx}
---@param dev_id string
---@param bank_index integer 0-based，对应 topic 中的 bank 序号
local function publish_bank_racks(dev_id, bank_index)
    local points = dc.read_all(dev_id)
    local yc_racks = split_bank_racks(points, RACK_YC_BASE_ID, RACK_YC_SPAN)
    local yx_racks = split_bank_racks(points, RACK_YX_BASE_ID, RACK_YX_SPAN)
    for i = 1, RACK_COUNT do
        local rack_index = i - 1
        publish("/pds/bank/" .. bank_index .. "/rack/" .. rack_index .. "/yc", yc_racks[i])
        publish("/pds/bank/" .. bank_index .. "/rack/" .. rack_index .. "/yx", yx_racks[i])
    end
end

task.spawn(function ()
	connect_mqtt()
end)

timer.every(2000,function ()
    if not conn then
        log.warn("MQTT 未连接，跳过本次上送")
        return
    end
    local yc_data, yx_data = crd()
    publish("/pds/crd/0/yc", yc_data)
    publish("/pds/crd/0/yx", yx_data)

    local pcs1_yc, pcs1_yt, pcs1_yx, pcs2_yc, pcs2_yt, pcs2_yx = pcs()
    publish("/pds/pcs/0/yc", pcs1_yc)
    publish("/pds/pcs/0/yt", pcs1_yt)
    publish("/pds/pcs/0/yx", pcs1_yx)
    publish("/pds/pcs/1/yc", pcs2_yc)
    publish("/pds/pcs/1/yt", pcs2_yt)
    publish("/pds/pcs/1/yx", pcs2_yx)

    local bau1_yc, bau1_yx, bau1_yt, bau2_yc, bau2_yx, bau2_yt = bank()
    publish("/pds/bank/0/yc", bau1_yc)
    publish("/pds/bank/0/yx", bau1_yx)
    publish("/pds/bank/0/yt", bau1_yt)
    publish("/pds/bank/1/yc", bau2_yc)
    publish("/pds/bank/1/yx", bau2_yx)
    publish("/pds/bank/1/yt", bau2_yt)

    publish_bank_racks("bau1", 0)
    publish_bank_racks("bau2", 1)
end)
