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

-- 英博2.5MW PCS采集点 -> 后端PcsYC标准点。
-- 协议给的是三路线电压，后端历史模型字段名仍为A/B/C相电压；这里保留三路顺序，后续应统一模型命名。
local PCS_YC_MAP = {
    [2] = { 1 }, [3] = { 2 }, [4] = { 3 },
    [5] = { 4 }, [6] = { 5 }, [7] = { 6 },
    [17] = { 7 },
    [18] = { 11 }, [19] = { 15 }, [20] = { 23 },
    [8] = { 24 }, [9] = { 25 }, [10] = { 26 },
    [1] = { 29 },
    [23] = { 30 }, [24] = { 31 },
}

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
local PCS_YT_TO_PROTOCOL = {
    [2001] = 2001, -- 运行模式选择
    [2004] = 2002, -- 并网有功给定
    [2005] = 2003, -- 并网无功给定
    [2007] = 2005, -- VF电压给定
    [2008] = 2006, -- VF频率给定
    [2031] = 2004, -- 并网功率因数
}

local PCS_PROTOCOL_TO_YT = {}
for std_id, protocol_id in pairs(PCS_YT_TO_PROTOCOL) do
    PCS_PROTOCOL_TO_YT[protocol_id] = { std_id }
end

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

local function map_comm_status(raw)
    for _, point in ipairs(raw or {}) do
        if point.id == COMM_STATUS_ID then
            -- dc.set_comm(true) 表示通讯正常；后端1001是“通讯诊断”告警位，故需要取反。
            local normal = point.value == 1 or point.value == true or point.value == "1"
            return { { id = 1001, value = normal and 0 or 1 } }
        end
    end
    -- 尚未获得任何通讯状态时按故障上送，避免界面误显示正常。
    return { { id = 1001, value = 1 } }
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
    local pcs_raw = dc.read_all("pcs") or {}
    local pcs_yc = map_points(pcs_raw, PCS_YC_MAP)
    if #pcs_yc > 0 then
        table.insert(routes, { topic = "/pds/pcs/0/yc", points = pcs_yc })
    end
    table.insert(routes, { topic = "/pds/pcs/0/yx", points = map_comm_status(pcs_raw) })
    local pcs_yt = map_points(pcs_raw, PCS_PROTOCOL_TO_YT)
    if #pcs_yt > 0 then
        table.insert(routes, { topic = "/pds/pcs/0/yt", points = pcs_yt })
    end

    -- BAU堆级数据：发布到后端实际使用的emu/0模型。
    local bau_raw = dc.read_all("bau") or {}
    local emu_yc = map_points(bau_raw, BAU_EMU_YC_MAP)
    local voltage = point_value(bau_raw, 50)
    local current = point_value(bau_raw, 51)
    if voltage ~= nil and current ~= nil then
        add_point(emu_yc, 30, voltage * current / 1000) -- kW，符号沿用BMS电流方向
    end
    if #emu_yc > 0 then
        table.insert(routes, { topic = "/pds/emu/0/yc", points = emu_yc })
    end
    table.insert(routes, { topic = "/pds/emu/0/yx", points = map_comm_status(bau_raw) })

    -- 12个BCU分别作为12个Rack实例，不合并，便于拓扑和设备页逐簇绑定。
    for i = 1, 12 do
        local raw = dc.read_all("bcu" .. i) or {}
        local rack_yc = map_points(raw, BCU_RACK_YC_MAP)
        if #rack_yc > 0 then
            table.insert(routes, { topic = "/pds/rack/" .. (i - 1) .. "/yc", points = rack_yc })
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
    conn:subscribe("/asw/pcs/0/setyt", function(_, payload)
        handle_mapped_downlink("pcs", PCS_YT_TO_PROTOCOL, payload)
    end)
    -- 后端当前会向此主题发送RackYT 2103跳机指令，但协能协议只有一键并网、复位和电操控制，
    -- 尚不能证明哪一个与“跳机”语义及取值完全一致。先明确拒绝，待联调确认后再加入白名单。
    conn:subscribe("/asw/bms/0/setyt", function(_, payload)
        log.warn("收到BMS控制但尚未建立经现场确认的协能BAU控制映射，已拒绝执行: " .. tostring(payload))
    end)
end

return M
