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

    -- 刷新下行反查表
    local function refresh_yt_maps()
        if project.GAOTE_BANK_YT_MAP then
            bau1_yt_map = {}
            for orig_id, std_id in pairs(project.GAOTE_BANK_YT_MAP) do
                bau1_yt_map[std_id] = { dev = "bau1", id = orig_id }
            end
            bau2_yt_map = {}
            for orig_id, std_id in pairs(project.GAOTE_BANK_YT_MAP) do
                bau2_yt_map[std_id] = { dev = "bau2", id = orig_id }
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

    local function map_bank_yx(raw_yx, comm_pt)
        local val_map = {}
        for _, p in ipairs(raw_yx) do
            if p.id ~= nil then val_map[p.id] = p.value end
        end
        local l1 = any_triggered(val_map, project.BANK_L1_IDS or {})
        local l2 = any_triggered(val_map, project.BANK_L2_IDS or {})
        local l3 = any_triggered(val_map, project.BANK_L3_IDS or {})
        local no_chg = (val_map[1053] == 1 or val_map[1053] == true or val_map[1053] == "1") and 1 or 0
        local no_dischg = (val_map[1054] == 1 or val_map[1054] == true or val_map[1054] == "1") and 1 or 0
        local result = {
            { id = 1007, value = l1 },
            { id = 1008, value = l2 },
            { id = 1009, value = l3 },
            { id = 1010, value = no_chg },
            { id = 1011, value = no_dischg },
        }
        if comm_pt then table.insert(result, comm_pt) end
        return result
    end

    local function collect_bank()
        local bau1 = dc.read_all("bau1")
        local bau2 = dc.read_all("bau2")

        local b1_comm = get_comm_point(bau1)
        local b2_comm = get_comm_point(bau2)

        local split_bank = function(points)
            local yc = utils.filter(points, function(p) return p.id ~= nil and p.id >= 20000 and p.id <= 20040 end)
            local yx = utils.filter(points, function(p) return p.id ~= nil and p.id >= 1000 and p.id <= 1079 end)
            local yt = utils.filter(points, function(p) return p.id ~= nil and p.id >= 50000 and p.id <= 50015 end)
            return yc, yx, yt
        end

        local b1_yc, b1_yx, b1_yt = split_bank(bau1)
        local b2_yc, b2_yx, b2_yt = split_bank(bau2)

        if project.GAOTE_BANK_YC_MAP then
            b1_yc = utils.map_points(b1_yc, project.GAOTE_BANK_YC_MAP)
            b2_yc = utils.map_points(b2_yc, project.GAOTE_BANK_YC_MAP)
        end

        b1_yx = map_bank_yx(b1_yx, b1_comm)
        b2_yx = map_bank_yx(b2_yx, b2_comm)

        if project.GAOTE_BANK_YT_MAP then
            b1_yt = utils.map_points(b1_yt, project.GAOTE_BANK_YT_MAP)
            b2_yt = utils.map_points(b2_yt, project.GAOTE_BANK_YT_MAP)
        end

        refresh_yt_maps()
        return b1_yc, b1_yx, b1_yt, b2_yc, b2_yx, b2_yt
    end

    local function publish_bank_racks(dev_id, bank_index)
        local points = dc.read_all(dev_id)
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

            local start_yx = yx_base + i * 1000
            local raw_yx = utils.filter(points, function(p) return p.id ~= nil and p.id >= start_yx and p.id <= start_yx + yx_span end)
            local mapped_yx = {}
            if project.RACK_L1_OFFSETS and project.RACK_L2_OFFSETS and project.RACK_L3_OFFSETS then
                local offset_map = {}
                for _, p in ipairs(raw_yx) do
                    offset_map[p.id - start_yx] = p.value
                end
                local l1 = any_triggered(offset_map, project.RACK_L1_OFFSETS)
                local l2 = any_triggered(offset_map, project.RACK_L2_OFFSETS)
                local l3 = any_triggered(offset_map, project.RACK_L3_OFFSETS)
                local yx_offset = i * 115
                mapped_yx = {
                    { id = 1203 + yx_offset, value = l1 },
                    { id = 1204 + yx_offset, value = l2 },
                    { id = 1205 + yx_offset, value = l3 },
                }
            else
                mapped_yx = raw_yx
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
        publish_bank_racks("bau1", 0)
        publish_bank_racks("bau2", 1)
    end)
end

return Engine
