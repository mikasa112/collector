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

--- 高特 BAU 堆遥测点位 -> 后端固定核心点位映射字典
--- 无法映射的核心点位不进入映射表，后端对应字段自然为空，不予展示
local GAOTE_BANK_YC_MAP = {
    [20000] = 41, -- 电池堆电操状态 -> 41: 堆充放电标定/复位状态
    [20001] = 4,  -- 电池堆电压 -> 4: 电池堆总电压
    [20002] = 5,  -- 电池堆电流 -> 5: 电池堆总电流
    [20003] = 3,  -- 电池堆 SOC -> 3: 电池堆SOC
    [20004] = 6,  -- 电池堆 SOH -> 6: 电池堆SOH
    [20005] = 9,  -- 最高电池电压 -> 9: 堆最高单体电压
    [20006] = 70, -- 最高电压电池组号 -> 70: 最高电压电池组号
    [20007] = 10, -- 最高电压电池所在组中的点号 -> 10: 堆最高单体电压编号
    [20008] = 11, -- 最低电池电压 -> 11: 堆最低单体电压
    [20009] = 71, -- 最低电压电池组号 -> 71: 最低电压电池组号
    [20010] = 12, -- 最低电压电池所在组中的点号 -> 12: 堆最低单体电压编号
    [20011] = 17, -- 最高电池温度 -> 17: 堆最高单体温度
    [20012] = 72, -- 最高温度电池组号 -> 72: 最高温度电池组号
    [20013] = 18, -- 最高温度电池所在组中的点号 -> 18: 堆最高单体温度编号
    [20014] = 19, -- 最低电池温度 -> 19: 堆最低单体温度
    [20015] = 73, -- 最低温度电池组号 -> 73: 最低温度电池组号
    [20016] = 20, -- 最低温度电池所在组中的点号 -> 20: 堆最低单体温度编号
    [20017] = 39, -- 堆累计充电电量 -> 39: 堆累计充电电量
    [20018] = 38, -- 堆累计放电电量 -> 38: 堆累计放电电量
    [20021] = 35, -- 堆可充电量 -> 35: 堆可充电量
    [20022] = 34, -- 堆可放电量 -> 34: 堆可放电量
    [20025] = 28, -- 允许最大放电功率 -> 28: 堆最大允许放电功率
    [20026] = 29, -- 允许最大充电功率 -> 29: 堆最大允许充电功率
    [20027] = 31, -- 允许最大放电电流 -> 31: 堆最大允许放电电流
    [20028] = 30, -- 允许最大充电电流 -> 30: 堆最大允许充电电流
    [20031] = 36, -- 当天放电电量 -> 36: 堆日放电电量
    [20032] = 37, -- 当天充电电量 -> 37: 堆日充电电量
    [20033] = 8,  -- 运行温度 -> 8: 堆单体平均温度 / 运行温度
    [20036] = 27, -- 电池堆绝缘电阻 -> 27: 电池堆绝缘阻值
}

--- 高特 簇遥测（相对于各簇 base_id 的偏移量）-> 后端固定核心点位映射字典
local GAOTE_RACK_YC_OFFSET_MAP = {
    [1]  = 127, -- 允许充电最大功率 -> 127: 簇最大允许充电功率
    [2]  = 126, -- 允许放电最大功率 -> 126: 簇最大允许放电功率
    [15] = 101, -- 组电压 -> 101: 电池簇电压
    [16] = 102, -- 组电流 -> 102: 电池簇电流
    [18] = 100, -- 组 SOC -> 100: 电池簇SOC
    [19] = 103, -- 组 SOH -> 103: 电池簇SOH
    [20] = 124, -- 组绝缘电阻 -> 124: 簇正母线对地绝缘阻值
    [21] = 104, -- 平均单体电压 -> 104: 簇单体平均电压
    [22] = 105, -- 平均单体温度 -> 105: 簇单体平均温度
    [23] = 106, -- 最高单体电压 -> 106: 簇最高单体电压
    [24] = 107, -- 最高单体电压对应点号 -> 107: 簇最高单体电压编号
    [25] = 108, -- 最低单体电压 -> 108: 簇最低单体电压值
    [26] = 109, -- 最低单体电压对应点号 -> 109: 簇最低单体电压编号
    [27] = 114, -- 最高单体温度 -> 114: 簇最高单体温度
    [28] = 115, -- 最高单体温度对应点号 -> 115: 簇最高单体温度编号
    [29] = 116, -- 最低单体温度 -> 116: 簇最低单体温度
    [30] = 117, -- 最低单体温度对应点号 -> 117: 簇最低单体温度编号
    [31] = 139, -- 最高单体SOC -> 139: 簇最高单体SOC
    [33] = 140, -- 最低单体SOC -> 140: 簇最低单体SOC
    [39] = 136, -- 累计充电电量 -> 136: 簇累计充电电量
    [40] = 135, -- 累计放电电量 -> 135: 簇累计放电电量
    [43] = 138, -- 可充电量 -> 138: 簇可充电量
    [44] = 137, -- 可放电量 -> 137: 簇可放电量
}

--- 高特 遥调点位 -> 后端固定核心点位映射字典
local GAOTE_BANK_YT_MAP = {
    [50001] = 2002, -- 故障复归 -> 2002
    [50002] = 2003, -- 接触器控制（预留） -> 2003
    [50003] = 2000, -- 系统开关机 -> 2000
    [50004] = 2006, -- 簇1启停 -> 2006
    [50005] = 2007, -- 簇2启停 -> 2007
    [50006] = 2008, -- 簇3启停 -> 2008
    [50007] = 2009, -- 簇4启停 -> 2009
    [50008] = 2010, -- 簇5启停 -> 2010
    [50009] = 2011, -- 簇6启停 -> 2011
    [50010] = 2019, -- 簇 7 维护模式控制 -> 2019: 簇7启停
    [50011] = 2020, -- 簇 8 维护模式控制 -> 2020: 簇8启停
    [50012] = 2021, -- 簇 9 维护模式控制 -> 2021: 簇9启停
    [50013] = 2022, -- 簇 10 维护模式控制 -> 2022: 簇10启停
    [50014] = 2023, -- 簇 11 维护模式控制 -> 2023: 簇11启停
    [50015] = 2024, -- 簇 12 维护模式控制 -> 2024: 簇12启停
}

--- 堆遥信分类集合（按严重级别与告警类型归类）
local BANK_L1_IDS = {
    1002, 1005, 1008, 1011, 1014, 1017, 1020, 1023, 1026, 1029,
    1032, 1035, 1038, 1041, 1044, 1047, 1048, 1049, 1050, 1051,
    1052, 1056, 1071, 1074, 1077, 1078, 1079
}
local BANK_L2_IDS = {
    1001, 1004, 1007, 1010, 1013, 1016, 1019, 1022, 1025, 1028,
    1031, 1034, 1037, 1040, 1043, 1046, 1055, 1070, 1073, 1076
}
local BANK_L3_IDS = {
    1000, 1003, 1006, 1009, 1012, 1015, 1018, 1021, 1024, 1027,
    1030, 1033, 1036, 1039, 1042, 1045, 1069, 1072, 1075
}

--- 簇遥信分类集合（相对于各簇起始点位的偏移 0..87）
local RACK_L1_OFFSETS = {
    0, 3, 6, 9, 12, 15, 18, 21, 24, 27, 30, 33, 36,
    37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50,
    51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63, 64,
    65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76,
    79, 82, 85, 86, 87
}
local RACK_L2_OFFSETS = {
    2, 5, 8, 11, 14, 17, 20, 23, 26, 29, 32, 35, 78, 81, 84
}
local RACK_L3_OFFSETS = {
    1, 4, 7, 10, 13, 16, 19, 22, 25, 28, 31, 34, 77, 80, 83
}

--- 检查列表中任意点位是否触发（值为 1 或 true）
local function any_triggered(val_map, id_list)
    for _, id in ipairs(id_list) do
        local v = val_map[id]
        if v == 1 or v == true or v == "1" then
            return 1
        end
    end
    return 0
end

--- 提取设备的通讯状态点（id=0xFFFF / 65535）
--- 若采集引擎注入则使用其实际值，若未注入则根据 points 是否有数据推断（有数据为 0 正常，空则为 1 离线）
local function get_comm_point(points)
    for _, p in ipairs(points) do
        if p.id == COMM_STATUS_ID then
            return { id = COMM_STATUS_ID, value = (p.value == 1 or p.value == true or p.value == "1") and 1 or 0 }
        end
    end
    local val = (#points > 0) and 0 or 1
    return { id = COMM_STATUS_ID, value = val }
end

--- 堆遥信聚合映射：将高特原始遥信按严重级别与禁充/放标志聚合为标准点位，并追加通讯状态点 65535
--- 1007: 堆一级故障, 1008: 堆二级告警, 1009: 堆三级预警, 1010: 堆禁充标志, 1011: 堆禁放标志, 65535: bank通讯诊断
local function map_bank_yx(raw_yx, comm_pt)
    local val_map = {}
    for _, p in ipairs(raw_yx) do
        if p.id ~= nil then
            val_map[p.id] = p.value
        end
    end

    local l1 = any_triggered(val_map, BANK_L1_IDS)
    local l2 = any_triggered(val_map, BANK_L2_IDS)
    local l3 = any_triggered(val_map, BANK_L3_IDS)
    local no_chg = (val_map[1053] == 1 or val_map[1053] == true or val_map[1053] == "1") and 1 or 0
    local no_dischg = (val_map[1054] == 1 or val_map[1054] == true or val_map[1054] == "1") and 1 or 0

    local result = {
        { id = 1007, value = l1 },
        { id = 1008, value = l2 },
        { id = 1009, value = l3 },
        { id = 1010, value = no_chg },
        { id = 1011, value = no_dischg },
    }
    if comm_pt then
        table.insert(result, comm_pt)
    end
    return result
end

--- 分别读取 bau1/bau2 的遥测(yc)、遥信(yx)与遥调(yt)数据，各自独立上送，不合并编号。
--- yc 映射为后端标准核心点位编号；
--- yx 聚合映射为后端标准核心告警点位（1007-1011）并附带通讯诊断点（65535）；
--- yt 映射后记录到 bau1_yt_map/bau2_yt_map，供下行 dc.dispatch 时反向映射回原始点位。
---@return DataPoint[] bau_yc
---@return DataPoint[] bau_yx
---@return DataPoint[] bau_yt
local function bank()
    local bau1 = dc.read_all("bau")

    local bau1_comm = get_comm_point(bau1)

    local bau1_yc, bau1_yx, bau1_yt = split_bank_yc_yx_yt(bau1)

    bau1_yc = utils.map_points(bau1_yc, GAOTE_BANK_YC_MAP)

    bau1_yx = map_bank_yx(bau1_yx, bau1_comm)

    bau1_yt = utils.map_points(bau1_yt, GAOTE_BANK_YT_MAP)

    bau1_yt_map = {}
    for orig_id, std_id in pairs(GAOTE_BANK_YT_MAP) do
        bau1_yt_map[std_id] = { dev = "bau", id = orig_id }
    end
    return bau1_yc, bau1_yx, bau1_yt
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
---@return DataPoint[][] racks 按簇序号(0-based，对应 topic 里的簇号)排列的数组，共 RACK_COUNT 组
local function split_bank_racks(points, base_id, span)
    local racks = {}
    for i = 0, RACK_COUNT - 1 do
        local start_id = base_id + i * 1000
        local end_id = start_id + span
        local raw_rack = utils.filter(points, function(p)
            return p.id ~= nil and p.id >= start_id and p.id <= end_id
        end)
        if base_id == RACK_YC_BASE_ID then
            local mapped_rack = {}
            for _, p in ipairs(raw_rack) do
                local offset = p.id - start_id
                local std_id = GAOTE_RACK_YC_OFFSET_MAP[offset]
                if std_id ~= nil then
                    local pt = {}
                    for k, v in pairs(p) do pt[k] = v end
                    -- 加上 i * 39 配合后端 MqttConfig.pointOffset 还原回基准
                    pt.id = std_id + i * 39
                    table.insert(mapped_rack, pt)
                end
            end
            racks[i + 1] = mapped_rack
        elseif base_id == RACK_YX_BASE_ID then
            -- 簇遥信聚合映射：L1(1203), L2(1204), L3(1205)
            -- 加上 i * 115 配合后端 MqttConfig.pointOffset 还原回基准
            local offset_map = {}
            for _, p in ipairs(raw_rack) do
                local offset = p.id - start_id
                offset_map[offset] = p.value
            end
            local l1 = any_triggered(offset_map, RACK_L1_OFFSETS)
            local l2 = any_triggered(offset_map, RACK_L2_OFFSETS)
            local l3 = any_triggered(offset_map, RACK_L3_OFFSETS)
            local yx_offset = i * 115
            racks[i + 1] = {
                { id = 1203 + yx_offset, value = l1 },
                { id = 1204 + yx_offset, value = l2 },
                { id = 1205 + yx_offset, value = l3 },
            }
        else
            racks[i + 1] = raw_rack
        end
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

        -- 单体电压(偏移45)/单体温度(偏移46)在 datacenter 中是 Val::List（每节电芯一个值），
        -- 不能并入上面按簇偏移逐点映射的标量负载(GAOTE_RACK_YC_OFFSET_MAP 未收录这两个偏移)，
        -- 需整体（数组）单独发布，key 沿用后端约定的 500(单体电压)/506(单体温度)。
        local start_yc = RACK_YC_BASE_ID + rack_index * 1000
        local cell_data = nil
        for _, p in ipairs(points) do
            if p.id == start_yc + 45 and p.value ~= nil then
                cell_data = cell_data or {}
                cell_data["500"] = p.value
            elseif p.id == start_yc + 46 and p.value ~= nil then
                cell_data = cell_data or {}
                cell_data["506"] = p.value
            end
        end
        if cell_data then
            publish("/pds/bank/" .. bank_index .. "/cell/" .. rack_index .. "/yc", cell_data)
        end
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

    local bau_yc, bau_yx, bau_yt = bank()
    publish("/pds/bank/1/yc", bau_yc)
    publish("/pds/bank/1/yx", bau_yx)
    publish("/pds/bank/1/yt", bau_yt)

    publish_bank_racks("bau", 1)
    -- publish_bank_racks("bau2", 1)
end)
