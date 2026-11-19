MOD = {
    name = "万润项目上云",
    description = "云平台：英博大机PCS(pcs)、协能BAU(bau)MQTT上送/下行，设备唯一sn",
}

-- 协议沿用 Backup/mqtt.lua：
--   上送 /gateway/prod/v1/{sn}/up   压缩发布 { sn, timestamp, data = { {id,key,value}, ... } }
--   下行 /gateway/prod/v1/{sn}/down { data = { {id=2002, value=100}, ... } }
-- 点号直接沿用各设备点表的序号：
--   PCS config/pcs/PCS_英博_大机.xlsx：1..71 遥测/遥信（启用点），2000..2029 遥调回读/下发
--   BAU config/bms/BAU_协能.xlsx：1..108 遥测/遥信，2000..2171 遥调回读/下发
--   BCU config/bms/BCU_协能.xlsx：设备 bcu1..bcu12，1..137 遥测/遥信，200..201 电芯，2000..2224 遥调回读/下发
--   电表 527AMC电表_400V_万润.lua：设备 amc_meter_1..amc_meter_11，点号 1..28，只采集上送，不接受下行

local HOST = "inpowergrid.com"
local PORT = 1883
local USERNAME = "root"
local PASSWORD = "inpower"
local CLIENT_ID ="wanrun_ems_cloud"

-- dev_id 必须与 config.json devices 中的 PCS 设备ID一致；sn 为云平台分配的唯一标识码（需按实际填写）
local DEVICE_SN = {
    pcs = "WAN_RUN_EMS@PCS",
    bau = "WAN_RUN_EMS@BAU", -- TODO: 按云平台实际分配的 sn 填写
}

-- BCU 簇：设备 bcu1..bcu12，sn 为 WAN_RUN_EMS@BCU1..12（TODO: 按云平台实际分配的 sn 修改）
local BCU_COUNT = 12
for n = 1, BCU_COUNT do
    DEVICE_SN["bcu" .. n] = "WAN_RUN_EMS@BCU" .. n
end

-- AMC 电表：设备 amc_meter_1..11，sn 为 WAN_RUN_EMS@METER1..11（TODO: 按云平台实际分配的 sn 修改）
-- 电表纯采集，不在 YT_RANGE 中登记，云端下行会被拒绝
local AMC_METER_COUNT = 11
for n = 1, AMC_METER_COUNT do
    DEVICE_SN["amc_meter_" .. n] = "WAN_RUN_EMS@METER" .. n
end

-- 允许云端下发的点号范围（遥调区），按设备区分
local YT_RANGE = {
    pcs = { 2000, 2029 },
    bau = { 2000, 2171 },
}
for n = 1, BCU_COUNT do
    YT_RANGE["bcu" .. n] = { 2000, 2224 }
end

local UPLOAD_INTERVAL_MS = 1000*60

---@type MqttConn
local conn = nil

-- sn -> dev_id 反查表
local SN_TO_DEV = {}
for dev_id, sn in pairs(DEVICE_SN) do
    SN_TO_DEV[sn] = dev_id
end

-- 下行统一用单层通配符订阅一次，避免连续多次 subscribe 导致请求通道发送失败
local DOWN_TOPIC_FILTER = "/gateway/prod/v1/+/down"

local function up_topic(sn)
    return "/gateway/prod/v1/" .. sn .. "/up"
end

--- 处理云端下行控制消息，从 topic 解析 sn 反查目标设备
local function handle_downlink(topic, payload)
    local sn = topic:match("^/gateway/prod/v1/([^/]+)/down$")
    local dev_id = sn and SN_TO_DEV[sn]
    if not dev_id then
        log.warn("mqtt 下行 topic 无法识别设备: " .. tostring(topic))
        return
    end
    local ok, msg = pcall(json.decode, payload)
    if not ok or type(msg) ~= "table" then
        log.warn("mqtt 下行消息解析失败 dev=" .. dev_id .. ": " .. tostring(msg))
        return
    end
    if type(msg.data) ~= "table" then
        log.warn("mqtt 下行消息格式错误 dev=" .. dev_id .. ": " .. tostring(payload))
        return
    end
    for _, item in ipairs(msg.data) do
        local id = tonumber(item.id)
        local rng = YT_RANGE[dev_id]
        if not id or not rng or id < rng[1] or id > rng[2] then
            log.warn("mqtt 下行点号不在允许范围 dev=" .. dev_id .. " id=" .. tostring(item.id))
        else
            local ok2, err = pcall(dc.dispatch, dev_id, id, item.value)
            if not ok2 then
                log.warn("mqtt 下行下发失败 dev=" .. dev_id .. " id=" .. id .. ": " .. tostring(err))
            end
        end
    end
end

--- 建立MQTT连接，失败自动重试；订阅一次通配符下行 topic
local function connect_mqtt()
    while true do
        local c, err = mqtt.connect({
            host = HOST,
            port = PORT,
            client_id = CLIENT_ID,
            username = USERNAME,
            password = PASSWORD,
            max_packet_size = 1024 * 256,
        })
        if c then
            conn = c
            log.info("mqtt 连接成功: " .. HOST .. ":" .. PORT)
            local ok, sub_err = pcall(function()
                conn:subscribe(DOWN_TOPIC_FILTER, handle_downlink)
            end)
            if not ok then
                log.warn("mqtt 订阅下行 topic 失败: " .. tostring(sub_err))
            end
            return
        end
        log.warn("mqtt 连接失败: " .. tostring(err) .. "，5秒后重试")
        wait(5000)
    end
end

task.spawn(function()
    connect_mqtt()
end)

--- 读取单个设备的全量数据点；无值的点不上送，避免云端被 null 覆盖
---@param dev_id string
---@return table
local function collect_device(dev_id)
    local list = dc.read_all(dev_id)
    local points = {}
    for _, item in ipairs(list or {}) do
        if item.value ~= nil then
            points[#points + 1] = {
                id    = item.id,
                key   = item.key,
                value = item.value,
            }
        end
    end
    return points
end

-- 按设备分别上送全量数据，每个设备使用各自的 sn 和 topic
timer.every(UPLOAD_INTERVAL_MS, function()
    if not conn then
        log.warn("mqtt 未连接，跳过本次上送")
        return
    end
    for dev_id, sn in pairs(DEVICE_SN) do
        local payload = {
            sn        = sn,
            timestamp = os.time(),
            data      = collect_device(dev_id),
        }
        local ok, err = pcall(function()
            conn:compressed_publish(up_topic(sn), payload, nil, 3)
        end)
        if not ok then
            log.warn("mqtt 上送失败 dev=" .. dev_id .. ": " .. tostring(err))
        end
    end
end)
