--- 通用北向流转引擎核心框架
--- 负责统一 MQTT 客户端生命周期、重连保活、下行指令调度解析、设备点位采集与上送

local utils = require("_utils")

local Engine = {}
local COMM_STATUS_ID = 0xFFFF

--- 裁剪数据点为 {"id": value, ...}，key 显式为 string 防止被 json 编码为数组
local function to_payload(points)
    local result = {}
    if points then
        for _, p in ipairs(points) do
            if p.id ~= nil and p.value ~= nil then
                result[tostring(p.id)] = p.value
            end
        end
    end
    return result
end

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
local function get_comm_point(points)
    for _, p in ipairs(points) do
        if p.id == COMM_STATUS_ID then
            return { id = COMM_STATUS_ID, value = (p.value == 1 or p.value == true or p.value == "1") and 1 or 0 }
        end
    end
    local val = (#points > 0) and 0 or 1
    return { id = COMM_STATUS_ID, value = val }
end

--- 分离通讯状态点与普通数据
local function split_comm_status(points)
    local data = utils.filter(points, function(p) return p.id ~= COMM_STATUS_ID end)
    local comm = utils.filter(points, function(p) return p.id == COMM_STATUS_ID end)
    return data, comm
end

--- 启动引擎
---@param project table 项目专属规则配置
function Engine.start(project)
    local host = project.HOST or "127.0.0.1"
    local port = project.PORT or 1883
    ---@type MqttConn
    local conn = nil

    local bau1_yt_map = {}
    local bau2_yt_map = {}

    -- 刷新下行反查表（支持根据各自堆协议动态映射）
    local function refresh_yt_maps()
        local dev0 = project.BANK0_DEV_ID or "ems"
        local proto0 = project.BANK0_PROTOCOL or "ems"
        local dev1 = project.BANK1_DEV_ID or "bau"
        local proto1 = project.BANK1_PROTOCOL or "bau"

        -- 堆0 反查表
        bau1_yt_map = {}
        local map0 = (proto0 == "ems" and project.GAOTE_EMS_BANK_YT_MAP) or project.GAOTE_BANK_YT_MAP
        if map0 then
            for orig_id, std_id in pairs(map0) do
                bau1_yt_map[std_id] = { dev = dev0, id = orig_id }
            end
        end

        -- 堆1 反查表
        bau2_yt_map = {}
        local map1 = (proto1 == "ems" and project.GAOTE_EMS_BANK_YT_MAP) or project.GAOTE_BANK_YT_MAP
        if map1 then
            for orig_id, std_id in pairs(map1) do
                bau2_yt_map[std_id] = { dev = dev1, id = orig_id }
            end
        end
    end
    refresh_yt_maps()

    -- 遥调下行透传派发
    local function handle_set_yt(dev_id, payload)
        local ok, msg = pcall(json.decode, payload)
        if not ok or type(msg) ~= "table" then
            log.warn("MQTT 遥调下行消息格式错误 dev=" .. dev_id .. ": " .. tostring(payload))
            return
        end
        for id_str, value in pairs(msg) do
            local id = tonumber(id_str)
            if id ~= nil then
                local ok2, err = pcall(dc.dispatch, dev_id, id, value)
                if not ok2 then
                    log.warn("MQTT 遥调下发失败 dev=" .. dev_id .. " id=" .. tostring(id) .. ": " .. tostring(err))
                end
            end
        end
    end

    -- 遥调下行经反查表映射派发
    local function handle_set_yt_mapped(map, payload)
        local ok, msg = pcall(json.decode, payload)
        if not ok or type(msg) ~= "table" then
            log.warn("MQTT 遥调下行消息格式错误: " .. tostring(payload))
            return
        end
        for id_str, value in pairs(msg) do
            local id = tonumber(id_str)
            if id ~= nil then
                local target = map[id]
                if target then
                    local ok2, err = pcall(dc.dispatch, target.dev, target.id, value)
                    if not ok2 then
                        log.warn("MQTT 遥调下发失败 dev=" .. target.dev .. " id=" .. tostring(target.id) .. ": " .. tostring(err))
                    end
                end
            end
        end
    end

    -- 统一发布消息
    local function publish(topic, points)
        if not conn then return end
        local ok, err = pcall(function()
            conn:publish(topic, to_payload(points))
        end)
        if not ok then
            log.warn("MQTT 上送失败 topic=" .. topic .. ": " .. tostring(err))
        end
    end

    -- MQTT 连接任务
    local function connect_mqtt()
        while true do
            local c, err = mqtt.connect({
                host = host,
                port = port,
                max_packet_size = 1024 * 256
            })
            if c then
                conn = c
                log.info("通用引擎: EMS MQTT 连接成功 " .. host .. ":" .. port)
                pcall(function()
                    if project.TOPIC_SET_PCS1_YT then
                        conn:subscribe(project.TOPIC_SET_PCS1_YT, function(_, payload) handle_set_yt("pcs1", payload) end)
                    end
                    if project.TOPIC_SET_PCS2_YT then
                        conn:subscribe(project.TOPIC_SET_PCS2_YT, function(_, payload) handle_set_yt("pcs2", payload) end)
                    end
                    if project.TOPIC_SET_BANK1_YT then
                        conn:subscribe(project.TOPIC_SET_BANK1_YT, function(_, payload) handle_set_yt_mapped(bau1_yt_map, payload) end)
                    end
                    if project.TOPIC_SET_BANK2_YT then
                        conn:subscribe(project.TOPIC_SET_BANK2_YT, function(_, payload) handle_set_yt_mapped(bau2_yt_map, payload) end)
                    end
                end)
                return
            end
            log.warn("EMS MQTT 连接失败: " .. tostring(err) .. "，5秒后重试")
            wait(5000)
        end
    end

    -- 采集与映射逻辑
    local function collect_crd()
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

    local function collect_pcs()
        local pcs1 = dc.read_all("pcs1")
        local pcs2 = dc.read_all("pcs2")

        local pcs1_data, pcs1_comm = split_comm_status(pcs1)
        local pcs2_data, pcs2_comm = split_comm_status(pcs2)

        local split_pcs = function(points)
            local yc = utils.filter(points, function(p) return p.id ~= nil and p.id >= 1 and p.id <= 57 end)
            local yt = utils.filter(points, function(p) return p.id ~= nil and p.id >= 2000 and p.id <= 2029 end)
            local yx = utils.filter(points, function(p) return p.id ~= nil and p.id >= 58 and p.id <= 71 end)
            return yc, yt, yx
        end

        local p1_yc, p1_yt, p1_yx_rng = split_pcs(pcs1_data)
        local p2_yc, p2_yt, p2_yx_rng = split_pcs(pcs2_data)

        local p1_yx = utils.concat({ p1_yx_rng, pcs1_comm })
        local p2_yx = utils.concat({ p2_yx_rng, pcs2_comm })

        return p1_yc, p1_yt, p1_yx, p2_yc, p2_yt, p2_yx
    end

    local function is_truthy(v)
        return v == 1 or v == true or v == "1"
    end

    local function map_bank_yx(raw_yx, comm_pt)
        -- 高特 BAU 堆遥信：不再聚合成笼统的 1007-1011 一二三级/禁充/禁放汇总位，
        -- 只逐点透传原始点位 1000-1079（平移 +8000），对应 BankYX.java 新增字段 9000-9079，
        -- 故障列表直接显示具体故障名
        local result = {}
        for _, p in ipairs(raw_yx) do
            if p.id ~= nil and p.id >= 1000 and p.id <= 1079 then
                table.insert(result, { id = p.id + 8000, value = p.value })
            end
        end
        if comm_pt then table.insert(result, comm_pt) end
        return result
    end

    local function map_ems_bank_yx(raw_yx, comm_pt)
        local val_map = {}
        for _, p in ipairs(raw_yx) do
            if p.id ~= nil then val_map[p.id] = p.value end
        end
        local l1 = any_triggered(val_map, project.GAOTE_EMS_BANK_L1_IDS or {}) -- 轻度预警
        local l2 = any_triggered(val_map, project.GAOTE_EMS_BANK_L2_IDS or {}) -- 中度告警
        local l3 = any_triggered(val_map, project.GAOTE_EMS_BANK_L3_IDS or {}) -- 严重故障
        local no_chg_id = project.GAOTE_EMS_BANK_NO_CHG_ID or 1046
        local no_dischg_id = project.GAOTE_EMS_BANK_NO_DISCHG_ID or 1047
        local no_chg = is_truthy(val_map[no_chg_id]) and 1 or 0
        local no_dischg = is_truthy(val_map[no_dischg_id]) and 1 or 0
        local result = {
            { id = 1007, value = l3 }, -- 1007: 堆一级故障 (严重)
            { id = 1008, value = l2 }, -- 1008: 堆二级告警 (中度)
            { id = 1009, value = l1 }, -- 1009: 堆三级预警 (轻度)
            { id = 1010, value = no_chg },    -- 1010: 堆禁充标志
            { id = 1011, value = no_dischg }, -- 1011: 堆禁放标志
        }
        if comm_pt then table.insert(result, comm_pt) end
        return result
    end

    --- 读取单个堆的遥测、遥信、遥调（支持 ems 与 bau 双协议）
    local function parse_single_bank(dev_id, protocol)
        local raw = dc.read_all(dev_id)
        if not raw or #raw == 0 then
            if dev_id == "bau1" then raw = dc.read_all("ems") or dc.read_all("bau") or {} end
            if dev_id == "ems" then raw = dc.read_all("bau1") or dc.read_all("bau") or {} end
        end
        raw = raw or {}
        local comm_pt = get_comm_point(raw)

        if protocol == "ems" then
            -- 高特本地 EMS 处理分支 (点位 20..46 为堆遥测，1000..1052 为堆遥信)
            local yc_raw = utils.filter(raw, function(p) return p.id ~= nil and p.id >= 20 and p.id <= 46 end)
            local yx_raw = utils.filter(raw, function(p) return p.id ~= nil and p.id >= 1000 and p.id <= 1052 end)
            local yt_raw = utils.filter(raw, function(p) return p.id ~= nil and p.id >= 2000 and p.id <= 2015 end)

            local mapped_yc = project.GAOTE_EMS_BANK_YC_MAP and utils.map_points(yc_raw, project.GAOTE_EMS_BANK_YC_MAP) or yc_raw
            local mapped_yx = map_ems_bank_yx(yx_raw, comm_pt)
            local mapped_yt = project.GAOTE_EMS_BANK_YT_MAP and utils.map_points(yt_raw, project.GAOTE_EMS_BANK_YT_MAP) or yt_raw
            return mapped_yc, mapped_yx, mapped_yt
        else
            -- 高特 BAU 处理分支 (寄存器 20000..20040)
            local yc_raw = utils.filter(raw, function(p) return p.id ~= nil and p.id >= 20000 and p.id <= 20040 end)
            local yx_raw = utils.filter(raw, function(p) return p.id ~= nil and p.id >= 1000 and p.id <= 1079 end)
            local yt_raw = utils.filter(raw, function(p) return p.id ~= nil and p.id >= 50000 and p.id <= 50015 end)

            local mapped_yc = project.GAOTE_BANK_YC_MAP and utils.map_points(yc_raw, project.GAOTE_BANK_YC_MAP) or yc_raw
            local mapped_yx = map_bank_yx(yx_raw, comm_pt)
            local mapped_yt = project.GAOTE_BANK_YT_MAP and utils.map_points(yt_raw, project.GAOTE_BANK_YT_MAP) or yt_raw
            return mapped_yc, mapped_yx, mapped_yt
        end
    end

    local function collect_bank()
        local dev0 = project.BANK0_DEV_ID or "ems"
        local proto0 = project.BANK0_PROTOCOL or "ems"
        local dev1 = project.BANK1_DEV_ID or "bau"
        local proto1 = project.BANK1_PROTOCOL or "bau"

        local b1_yc, b1_yx, b1_yt = parse_single_bank(dev0, proto0)
        local b2_yc, b2_yx, b2_yt = parse_single_bank(dev1, proto1)

        refresh_yt_maps()
        return b1_yc, b1_yx, b1_yt, b2_yc, b2_yx, b2_yt
    end

    local function publish_bank_racks(dev_id, bank_index, protocol)
        local points = dc.read_all(dev_id)
        if not points or #points == 0 then
            if dev_id == "bau1" then points = dc.read_all("ems") or dc.read_all("bau") or {} end
            if dev_id == "ems" then points = dc.read_all("bau1") or dc.read_all("bau") or {} end
        end
        points = points or {}

        local proto = protocol or (bank_index == 0 and (project.BANK0_PROTOCOL or "ems") or (project.BANK1_PROTOCOL or "bau"))

        if proto == "ems" then
            -- 高特本地 EMS 簇解析分支 (12 簇，遥测每簇 30 点位，遥信每簇 60 点位)
            local rack_count = project.EMS_RACK_COUNT or 12
            local yc_base = project.EMS_RACK_YC_BASE_ID or 50
            local yc_step = project.EMS_RACK_YC_STEP or 30
            local yc_span = project.EMS_RACK_YC_SPAN or 24
            local yx_base = project.EMS_RACK_YX_BASE_ID or 1053
            local yx_step = project.EMS_RACK_YX_STEP or 60
            local yx_span = project.EMS_RACK_YX_SPAN or 53

            for i = 0, rack_count - 1 do
                local start_yc = yc_base + i * yc_step
                local raw_yc = utils.filter(points, function(p) return p.id ~= nil and p.id >= start_yc and p.id <= start_yc + yc_span end)
                local mapped_yc = {}
                if project.GAOTE_EMS_RACK_YC_OFFSET_MAP then
                    for _, p in ipairs(raw_yc) do
                        local offset = p.id - start_yc
                        local std_id = project.GAOTE_EMS_RACK_YC_OFFSET_MAP[offset]
                        if std_id ~= nil then
                            local pt = {}
                            for k, v in pairs(p) do pt[k] = v end
                            -- 与后端 pointOffset("rack", "yc", i) 步长 39 保持对齐
                            pt.id = std_id + i * 39
                            table.insert(mapped_yc, pt)
                        end
                    end
                else
                    mapped_yc = raw_yc
                end

                local start_yx = yx_base + i * yx_step
                local raw_yx = utils.filter(points, function(p) return p.id ~= nil and p.id >= start_yx and p.id <= start_yx + yx_span end)
                local offset_map = {}
                for _, p in ipairs(raw_yx) do
                    offset_map[p.id - start_yx] = p.value
                end

                local l1 = any_triggered(offset_map, project.GAOTE_EMS_RACK_L1_OFFSETS or {})
                local l2 = any_triggered(offset_map, project.GAOTE_EMS_RACK_L2_OFFSETS or {})
                local l3 = any_triggered(offset_map, project.GAOTE_EMS_RACK_L3_OFFSETS or {})
                local contactor = is_truthy(offset_map[project.GAOTE_EMS_RACK_CONTACTOR_OFFSET or 48]) and 1 or 0
                local no_chg = is_truthy(offset_map[project.GAOTE_EMS_RACK_NO_CHG_OFFSET or 49]) and 1 or 0
                local no_dischg = is_truthy(offset_map[project.GAOTE_EMS_RACK_NO_DISCHG_OFFSET or 50]) and 1 or 0

                local yx_offset = i * 115
                local mapped_yx = {
                    { id = 1203 + yx_offset, value = l3 },        -- 簇一级故障 (严重)
                    { id = 1204 + yx_offset, value = l2 },        -- 簇二级告警 (中度)
                    { id = 1205 + yx_offset, value = l1 },        -- 簇三级预警 (轻度)
                    { id = 1206 + yx_offset, value = no_chg },    -- 簇禁充标志
                    { id = 1207 + yx_offset, value = no_dischg }, -- 簇禁放标志
                    { id = 1208 + yx_offset, value = contactor }, -- 簇总正接触器状态
                }

                publish("/pds/bank/" .. bank_index .. "/rack/" .. i .. "/yc", mapped_yc)
                publish("/pds/bank/" .. bank_index .. "/rack/" .. i .. "/yx", mapped_yx)
            end
        else
            -- 高特 BAU 簇解析分支 (保留原有逻辑)
            local rack_count = project.RACK_COUNT or 12
            local yc_base = project.RACK_YC_BASE_ID or 21000
            local yc_span = project.RACK_YC_SPAN or 46
            local yx_base = project.RACK_YX_BASE_ID or 2000
            local yx_span = project.RACK_YX_SPAN or 87

            for i = 0, rack_count - 1 do
                local start_yc = yc_base + i * 1000
                local raw_yc = utils.filter(points, function(p) return p.id ~= nil and p.id >= start_yc and p.id <= start_yc + yc_span end)
                local mapped_yc = {}
                if project.GAOTE_RACK_YC_OFFSET_MAP then
                    for _, p in ipairs(raw_yc) do
                        local offset = p.id - start_yc
                        local std_id = project.GAOTE_RACK_YC_OFFSET_MAP[offset]
                        if std_id ~= nil then
                            local pt = {}
                            for k, v in pairs(p) do pt[k] = v end
                            pt.id = std_id + i * 39
                            table.insert(mapped_yc, pt)
                        end
                    end
                else
                    mapped_yc = raw_yc
                end

                -- 高特各簇原始基址不规律（簇1=2000,簇2=2100,簇3=3000,簇4起才是+1000规整），
                -- 不能用线性公式 yx_base + i*1000 推算，必须查表
                local start_yx = (project.GAOTE_RACK_YX_BASES and project.GAOTE_RACK_YX_BASES[i + 1]) or (yx_base + i * 1000)
                local raw_yx = utils.filter(points, function(p) return p.id ~= nil and p.id >= start_yx and p.id <= start_yx + yx_span end)
                -- 高特 BAU 簇遥信：不再聚合成笼统的 1203-1205 一二三级汇总位，
                -- 只逐点透传：原始点位相对本簇基址的偏移(0..87)映射到簇1基准 2000-2087，
                -- 再平移 +8000 得到 10000-10087，最后按本簇实例加 i*115（与后端 pointOffset("rack","yx",i) 对齐），
                -- 对应 RackYX.java 新增字段，故障列表直接显示具体故障名
                local yx_offset = i * 115
                local mapped_yx = {}
                for _, p in ipairs(raw_yx) do
                    local local_offset = p.id - start_yx
                    table.insert(mapped_yx, { id = 10000 + local_offset + yx_offset, value = p.value })
                end

                publish("/pds/bank/" .. bank_index .. "/rack/" .. i .. "/yc", mapped_yc)
                publish("/pds/bank/" .. bank_index .. "/rack/" .. i .. "/yx", mapped_yx)

                -- 提取并发布该簇的单体电压(500)与单体温度(506)
                local cell_vol_pt = utils.find(raw_yc, function(p) return p.id == start_yc + 45 end)
                local cell_temp_pt = utils.find(raw_yc, function(p) return p.id == start_yc + 46 end)
                if cell_vol_pt or cell_temp_pt then
                    local cell_data = {}
                    if cell_vol_pt and cell_vol_pt.value ~= nil then
                        cell_data["500"] = cell_vol_pt.value
                    end
                    if cell_temp_pt and cell_temp_pt.value ~= nil then
                        cell_data["506"] = cell_temp_pt.value
                    end
                    publish("/pds/bank/" .. bank_index .. "/cell/" .. i .. "/yc", cell_data)
                end
            end
        end
    end

    -- 启动连接与采集轮询
    task.spawn(connect_mqtt)

    timer.every(2000, function()
        if not conn then
            log.warn("通用引擎: MQTT 未连接，跳过本次上送")
            return
        end

        -- 1. CRD
        local crd_yc, crd_yx = collect_crd()
        publish("/pds/crd/0/yc", crd_yc)
        publish("/pds/crd/0/yx", crd_yx)

        -- 2. PCS
        local p1_yc, p1_yt, p1_yx, p2_yc, p2_yt, p2_yx = collect_pcs()
        publish("/pds/pcs/0/yc", p1_yc)
        publish("/pds/pcs/0/yt", p1_yt)
        publish("/pds/pcs/0/yx", p1_yx)
        publish("/pds/pcs/1/yc", p2_yc)
        publish("/pds/pcs/1/yt", p2_yt)
        publish("/pds/pcs/1/yx", p2_yx)

        -- 3. BANK
        local b1_yc, b1_yx, b1_yt, b2_yc, b2_yx, b2_yt = collect_bank()
        publish("/pds/bank/0/yc", b1_yc)
        publish("/pds/bank/0/yx", b1_yx)
        publish("/pds/bank/0/yt", b1_yt)
        publish("/pds/bank/1/yc", b2_yc)
        publish("/pds/bank/1/yx", b2_yx)
        publish("/pds/bank/1/yt", b2_yt)

        -- 4. RACKS
        local dev0 = project.BANK0_DEV_ID or "ems"
        local proto0 = project.BANK0_PROTOCOL or "ems"
        local dev1 = project.BANK1_DEV_ID or "bau"
        local proto1 = project.BANK1_PROTOCOL or "bau"
        publish_bank_racks(dev0, 0, proto0)
        publish_bank_racks(dev1, 1, proto1)
    end)
end

return Engine
