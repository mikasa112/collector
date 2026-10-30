-- 万润 5MWh 项目北向适配层。
--
-- 职责边界：
--   1. 527AMC电表_万润.lua 负责 DL/T645 采集，写入数据中心，点号是采集协议内部点号；
--   2. 本文件负责把采集点号映射为后端 MeterYC/PcsYC/EmuYC/RackYC 标准点号，并按四遥 Topic 发布；
--   3. 后端只消费稳定的 /pds/{model}/{index}/{yc|yx|yt|yk} 契约，不感知厂家协议点号。

MOD = {
    name = "万润5MWh项目北向",
    description = "万润设备点位到EMS标准四遥模型的MQTT适配",
}

local M = {}
M.MOD = MOD
M.HOST = "127.0.0.1"
M.PORT = 1883
M.USE_GENERIC_ROUTES = true

-- PCS 型号默认跟随南向点表自动识别：
--   PCS_英博_125.xlsx  -> PCS_KW
--   PCS_英博_大机.xlsx -> PCS_MW
-- 不要为两种型号改 MQTT topic，后端会根据资产绑定的 PCS_KW / PCS_MW 选择物理模型。
-- 如果首轮采集尚未出现可识别的 key，使用 fallback；也可将 model 显式改成 PCS_KW/PCS_MW。
M.PCS = { dev = "pcs", index = 0, model = "AUTO", fallback = "PCS_MW" }

-- 电池仓与采集设备的对应关系。新增电池仓只需追加配置，北向Topic会自动携带仓号和簇号。
-- bau/bcus中的值必须与南向协议脚本注册到dc的dev_id一致。
M.BATTERY_CONTAINERS = {
    { index = 0, name = "1#电池仓", bau = "bau", bcus = {
        "bcu1", "bcu2", "bcu3", "bcu4", "bcu5", "bcu6",
        "bcu7", "bcu8", "bcu9", "bcu10", "bcu11", "bcu12",
    } },
}

-- 启动 527AMC 的南向采集。后续其它万润协议脚本也在此处 require，北向映射仍集中保留在本文件。
local ok, err = pcall(require, "projects.527AMC电表_万润")
if not ok then
    log.error("加载万润527AMC采集脚本失败: " .. tostring(err))
end

-- 527AMC 采集脚本内部点号：
-- 1组合有功电能、2正向有功电能、3反向有功电能、4/5无功电能、
-- 6..8电压、9..11电流、12..23功率、24..27功率因数、28频率、29/30变比。
-- 注意：这些点号不是后端 Java Model 点号，必须经过下面的显式映射。

-- 后端 MeterYC 标准点号。
local METER_YC_MAP = {
    [6] = { 1 }, [7] = { 2 }, [8] = { 3 },
    [9] = { 4 }, [10] = { 5 }, [11] = { 6 },
    [12] = { 8 }, [13] = { 9 }, [14] = { 10 }, [15] = { 11 },
    [16] = { 12 }, [17] = { 13 }, [18] = { 14 }, [19] = { 15 },
    [20] = { 16 }, [21] = { 17 }, [22] = { 18 }, [23] = { 19 },
    [24] = { 20 }, [25] = { 21 }, [26] = { 22 }, [27] = { 23 },
    -- 527只有一个系统频率；复制到 MeterYC 的A/B/C三相频率字段。
    [28] = { 27, 28, 29 },
    [2] = { 30 }, [3] = { 31 },
    -- EQ1/EQ2 的象限定义需按现场表计参数确认后，才能映射到32/33，当前不猜测。
}

-- kW 协议点表与后端 PcsYC 的前 33 点一致，后半段在厂家点表中穿插了版本、故障字和 BMS 点，
-- 因此必须按语义显式映射，不能直接透传数字。
local PCS_KW_YC_MAP = {}
for id = 1, 33 do PCS_KW_YC_MAP[id] = { id } end
PCS_KW_YC_MAP[35] = { 34 }; PCS_KW_YC_MAP[36] = { 35 }; PCS_KW_YC_MAP[37] = { 36 }
PCS_KW_YC_MAP[38] = { 37 }; PCS_KW_YC_MAP[39] = { 38 }; PCS_KW_YC_MAP[40] = { 39 }
PCS_KW_YC_MAP[59] = { 40 }; PCS_KW_YC_MAP[60] = { 41 }; PCS_KW_YC_MAP[61] = { 42 }
PCS_KW_YC_MAP[62] = { 43 }; PCS_KW_YC_MAP[63] = { 44 }; PCS_KW_YC_MAP[64] = { 45 }
PCS_KW_YC_MAP[133] = { 46 }; PCS_KW_YC_MAP[134] = { 47 }; PCS_KW_YC_MAP[135] = { 48 }
PCS_KW_YC_MAP[136] = { 49 }; PCS_KW_YC_MAP[137] = { 50 }; PCS_KW_YC_MAP[138] = { 51 }
for source = 139, 155 do PCS_KW_YC_MAP[source] = { source - 87 } end

local PCS_KW_YX_MAP = {
    [1000] = { 1002 }, [1001] = { 1003 }, [1002] = { 1004 }, [1003] = { 1005 },
    [1004] = { 1006 }, [1005] = { 1007 }, [1006] = { 1008 }, [1007] = { 1009 },
    [1008] = { 1010 }, [1009] = { 1011 }, [1013] = { 1012 },
    [156] = { 1014 }, [157] = { 1015 }, [158] = { 1016 }, [159] = { 1017 },
    [160] = { 1018 }, [164] = { 1019 }, [165] = { 1020 },
}

local PCS_KW_YT_FROM_PROTOCOL = {}
for source = 2000, 2016 do PCS_KW_YT_FROM_PROTOCOL[source] = { source + 1 } end
for source = 2018, 2020 do PCS_KW_YT_FROM_PROTOCOL[source] = { source } end
PCS_KW_YT_FROM_PROTOCOL[2022] = { 2022 }; PCS_KW_YT_FROM_PROTOCOL[2023] = { 2023 }
PCS_KW_YT_FROM_PROTOCOL[2028] = { 2024 }; PCS_KW_YT_FROM_PROTOCOL[2029] = { 2025 }
PCS_KW_YT_FROM_PROTOCOL[2030] = { 2026 }; PCS_KW_YT_FROM_PROTOCOL[2031] = { 2027 }
PCS_KW_YT_FROM_PROTOCOL[2032] = { 2028 }; PCS_KW_YT_FROM_PROTOCOL[2033] = { 2029 }
PCS_KW_YT_FROM_PROTOCOL[2034] = { 2030 }; PCS_KW_YT_FROM_PROTOCOL[2035] = { 2031 }
PCS_KW_YT_FROM_PROTOCOL[2039] = { 2032 }; PCS_KW_YT_FROM_PROTOCOL[2040] = { 2033 }
PCS_KW_YT_FROM_PROTOCOL[2041] = { 2034 }; PCS_KW_YT_FROM_PROTOCOL[2042] = { 2035 }

-- MW 大机点表就是后端 PcsMwYC/PcsMwYX/PcsMwYT 的物理模型，仅做四遥分类，点号不再改写。
local PCS_MW_YC_MAP, PCS_MW_YX_MAP, PCS_MW_YT_FROM_PROTOCOL = {}, {}, {}
for id = 1, 57 do PCS_MW_YC_MAP[id] = { id } end
for id = 58, 71 do PCS_MW_YX_MAP[id] = { id } end
for id = 2000, 2029 do PCS_MW_YT_FROM_PROTOCOL[id] = { id } end

-- 协能BAU堆级数据 -> 后端现有EmuYC标准点。
-- 当前策略、首页、收益计算均固定读取emu/0，因此堆级BMS必须落到该模型，而不是不可解析的bank主题。
local BAU_EMU_YC_MAP = {
    [61] = { 7 }, [63] = { 8 },
    [65] = { 9 }, [67] = { 10 },
    [64] = { 11 }, [68] = { 12 }, [69] = { 13 },
    [73] = { 14 }, [77] = { 15 }, [78] = { 16 },
    [70] = { 17 }, [74] = { 18 },
    [53] = { 19 }, [52] = { 20 },
    [79] = { 24 }, [80] = { 25 },
    [51] = { 28 }, [50] = { 29 },
    [57] = { 49 }, [58] = { 50 },
    [85] = { 51 }, [84] = { 52 },
}

-- 协能BCU簇级数据 -> 后端RackYC标准点。
local BCU_RACK_YC_MAP = {
    [66] = { 1 }, [67] = { 2 }, [56] = { 3 }, [57] = { 4 },
    [73] = { 6 }, [74] = { 7 }, [10] = { 8 },
    [43] = { 12 }, [45] = { 13 }, [46] = { 14 },
    [47] = { 15 }, [49] = { 16 }, [50] = { 17 }, [51] = { 18 },
    [42] = { 20 },
    [33] = { 21 }, [35] = { 22 }, [36] = { 23 },
    [37] = { 24 }, [39] = { 25 }, [40] = { 26 },
    [86] = { 59 }, [87] = { 60 },
    [11] = { 69 }, [5] = { 71 }, [1] = { 72 },
    [54] = { 75 }, [55] = { 76 },
    [34] = { 79 }, [38] = { 80 }, [44] = { 81 }, [48] = { 82 },
    -- 协能BCU协议中这两个点为数组：416节单体电压、208个温度采样点。
    [200] = { 121 }, [201] = { 122 },
}

-- 后端PcsYT标准点 -> 英博PCS协议遥调点。
-- 只开放语义完全一致的点位；启动、停机、待机等控制不得靠枚举猜测。
local PCS_KW_YT_TO_PROTOCOL = {}
for protocol_id, standard_ids in pairs(PCS_KW_YT_FROM_PROTOCOL) do
    PCS_KW_YT_TO_PROTOCOL[standard_ids[1]] = protocol_id
end
-- kW 机的启停、复位等是遥控 Coils，后端为了统一策略调用将其暴露为 PcsYT 2036..2043。
for standard_id = 2036, 2043 do PCS_KW_YT_TO_PROTOCOL[standard_id] = 3000 + standard_id - 2036 end

-- 万润现场只有1块400V电表，使用通用MeterYC模型，不冒充10kV表。
local METER_ROUTES = {
    { dev = "amc_meter_1", model = "meter", index = 0, yc_map = METER_YC_MAP },
}

local COMM_STATUS_ID = 0xFFFF

local function map_points(raw, point_map)
    local result = {}
    for _, point in ipairs(raw or {}) do
        local target_ids = point_map[point.id]
        if target_ids and point.value ~= nil then
            for _, target_id in ipairs(target_ids) do
                table.insert(result, { id = target_id, value = point.value })
            end
        end
    end
    return result
end

local function point_value(raw, id)
    for _, point in ipairs(raw or {}) do
        if point.id == id then return point.value end
    end
    return nil
end

local function add_point(points, id, value)
    if value ~= nil then table.insert(points, { id = id, value = value }) end
end

local function map_comm_status(raw, alarm_id)
    alarm_id = alarm_id or 1001
    for _, point in ipairs(raw or {}) do
        if point.id == COMM_STATUS_ID then
            -- dc.set_comm(true) 表示通讯正常；后端通讯诊断是告警位，故需要取反。
            local normal = point.value == 1 or point.value == true or point.value == "1"
            return { { id = alarm_id, value = normal and 0 or 1 } }
        end
    end
    -- 尚未获得任何通讯状态时按故障上送，避免界面误显示正常。
    return { { id = alarm_id, value = 1 } }
end

local function detect_pcs_model(raw)
    if M.PCS.model == "PCS_KW" or M.PCS.model == "PCS_MW" then return M.PCS.model end
    for _, point in ipairs(raw or {}) do
        -- key 来自南向 xlsx，比靠数值或单一点号判断更稳定。
        if point.key == "pcsPortAPhVolt" or point.key == "pcsCtrlSwFaultW1" then return "PCS_KW" end
        if point.key == "workStatus" or point.key == "maximumRechargeableActivePowerOfPcs" then return "PCS_MW" end
    end
    return M.PCS.fallback or "PCS_KW"
end

local function pcs_profile(model)
    if model == "PCS_MW" then
        return PCS_MW_YC_MAP, PCS_MW_YX_MAP, PCS_MW_YT_FROM_PROTOCOL, nil, COMM_STATUS_ID
    end
    return PCS_KW_YC_MAP, PCS_KW_YX_MAP, PCS_KW_YT_FROM_PROTOCOL, PCS_KW_YT_TO_PROTOCOL, 1001
end

function M.collect_routes()
    local routes = {}
    for _, route in ipairs(METER_ROUTES) do
        local raw = dc.read_all(route.dev) or {}
        local prefix = "/pds/" .. route.model .. "/" .. route.index
        local yc = map_points(raw, route.yc_map)
        -- 通讯异常或尚未完成首轮采集时不发布空遥测，避免后端用全null对象覆盖最后有效值。
        if #yc > 0 then
            table.insert(routes, { topic = prefix .. "/yc", points = yc })
        end
        table.insert(routes, { topic = prefix .. "/yx", points = map_comm_status(raw) })
    end

    -- PCS：遥测、通讯遥信以及遥调回读。
    local pcs_raw = dc.read_all(M.PCS.dev) or {}
    local pcs_model = detect_pcs_model(pcs_raw)
    local yc_map, yx_map, yt_map, _, comm_id = pcs_profile(pcs_model)
    local pcs_yc = map_points(pcs_raw, yc_map)
    if #pcs_yc > 0 then
        table.insert(routes, { topic = "/pds/pcs/" .. M.PCS.index .. "/yc", points = pcs_yc })
    end
    local pcs_yx = map_points(pcs_raw, yx_map)
    local comm = map_comm_status(pcs_raw, comm_id)
    for _, point in ipairs(comm) do table.insert(pcs_yx, point) end
    table.insert(routes, { topic = "/pds/pcs/" .. M.PCS.index .. "/yx", points = pcs_yx })
    local pcs_yt = map_points(pcs_raw, yt_map)
    if #pcs_yt > 0 then
        table.insert(routes, { topic = "/pds/pcs/" .. M.PCS.index .. "/yt", points = pcs_yt })
    end

    for _, container in ipairs(M.BATTERY_CONTAINERS or {}) do
        local container_index = container.index or 0
        local bau_raw = dc.read_all(container.bau) or {}
        local emu_yc = map_points(bau_raw, BAU_EMU_YC_MAP)
        local voltage = point_value(bau_raw, 50)
        local current = point_value(bau_raw, 51)
        if voltage ~= nil and current ~= nil then
            add_point(emu_yc, 30, voltage * current / 1000)
        end
        if #emu_yc > 0 then
            table.insert(routes, { topic = "/pds/bank/" .. container_index .. "/yc", points = emu_yc })
        end
        table.insert(routes, { topic = "/pds/bank/" .. container_index .. "/yx", points = map_comm_status(bau_raw) })

        for rack_index, bcu_dev_id in ipairs(container.bcus or {}) do
            local raw = dc.read_all(bcu_dev_id) or {}
            local rack_yc = map_points(raw, BCU_RACK_YC_MAP)
            local rack_prefix = "/pds/bank/" .. container_index .. "/rack/" .. (rack_index - 1)
            if #rack_yc > 0 then
                table.insert(routes, { topic = rack_prefix .. "/yc", points = rack_yc })
            end
            -- RackYX 1001是组端过压告警，不是通讯状态；待告警字位协议确认后单独映射YX。
        end
    end
    return routes
end

local function handle_mapped_downlink(device_id, mapping, payload)
    local ok_decode, message = pcall(json.decode, payload)
    if not ok_decode or type(message) ~= "table" then
        log.warn("万润下行消息格式错误 dev=" .. device_id .. ": " .. tostring(payload))
        return
    end
    for id_text, value in pairs(message) do
        local standard_id = tonumber(id_text)
        local protocol_id = standard_id and mapping[standard_id]
        if protocol_id then
            local ok_dispatch, dispatch_err = pcall(dc.dispatch, device_id, protocol_id, value)
            if not ok_dispatch then
                log.warn("万润下行失败 dev=" .. device_id .. " id=" .. tostring(protocol_id) .. ": " .. tostring(dispatch_err))
            end
        elseif value ~= nil and value ~= 0 then
            log.warn("万润下行点未建立安全映射 dev=" .. device_id .. " standard_id=" .. tostring(id_text))
        end
    end
end

function M.register_downlinks(conn)
    conn:subscribe("/asw/pcs/" .. M.PCS.index .. "/setyt", function(_, payload)
        local raw = dc.read_all(M.PCS.dev) or {}
        local model = detect_pcs_model(raw)
        local _, _, _, mapping = pcs_profile(model)
        if model == "PCS_MW" then
            -- 后端已按 PcsMwYT 转换成大机原始点号，采集层直接下发。
            local direct = {}
            for id = 2000, 2029 do direct[id] = id end
            handle_mapped_downlink(M.PCS.dev, direct, payload)
        else
            handle_mapped_downlink(M.PCS.dev, mapping, payload)
        end
    end)
    -- 后端当前会向此主题发送RackYT 2103跳机指令，但协能协议只有一键并网、复位和电操控制，
    -- 尚不能证明哪一个与“跳机”语义及取值完全一致。先明确拒绝，待联调确认后再加入白名单。
    conn:subscribe("/asw/bms/0/setyt", function(_, payload)
        log.warn("收到BMS控制但尚未建立经现场确认的协能BAU控制映射，已拒绝执行: " .. tostring(payload))
    end)
end

-- 只用于无设备的自动化契约测试，生产路径仍仅调用 collect_routes/register_downlinks。
M._test = {
    map_points = map_points,
    detect_pcs_model = detect_pcs_model,
    pcs_profile = pcs_profile,
}

return M
