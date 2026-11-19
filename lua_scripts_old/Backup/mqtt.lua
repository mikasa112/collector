MOD = {
    name        = "云端MQTT规则",
    description = "英博云平台MQTT上送/下行（按设备独立上送，每设备唯一sn）",
}

local HOST = "47.98.106.76"
local PORT = 1883

-- 每个设备的唯一标识码 sn，对应 config.json devices 下的 pcs/bcu/dehum/tms/fire/gpio
local DEVICE_SN = {
    pcs   = "INPOWEREMU@2PCS",
    bcu   = "INPOWEREMU@2BCU",
    dehum = "INPOWEREMU@2DEHUM",
    tms   = "INPOWEREMU@2TMS",
    fire  = "INPOWEREMU@2FIRE",
    gpio  = "INPOWEREMU@2GPIO",
}

---@type MqttConn
local conn = nil

-- sn -> dev_id 反查表，用于从下行 topic 中解析出的 sn 找到对应设备
local SN_TO_DEV = {}
for dev_id, sn in pairs(DEVICE_SN) do
    SN_TO_DEV[sn] = dev_id
end

-- 下行 topic 统一用单层通配符订阅一次，避免连接建立后连续多次 subscribe
-- 导致 rumqttc 请求通道发送失败（"Failed to send mqtt requests to eventloop"）
local DOWN_TOPIC_FILTER = "/gateway/prod/v1/+/down"

local function up_topic(sn)
    return "/gateway/prod/v1/" .. sn .. "/up"
end

--- 处理云端下行控制消息，从 topic 中解析 sn 反查出目标设备
--- payload 结构: { data = { {id=1, value=100}, ... } }
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
        local ok2, err = pcall(dc.dispatch, dev_id, item.id, item.value)
        if not ok2 then
            log.warn("mqtt 下行下发失败 dev=" .. dev_id .. " id=" .. tostring(item.id) .. ": " .. tostring(err))
        end
    end
end

--- 建立MQTT连接，断开/失败后自动重试；统一订阅一次通配符下行 topic
local function connect_mqtt()
    while true do
        local c, err = mqtt.connect({
            host = HOST,
            port = PORT,
            client_id = "lang_fang_emu2",
            max_packet_size = 1024 * 256
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

-- bcu 设备需要排除的点位 ID
local BCU_EXCLUDE_IDS = { [2000] = true, [2001] = true }

--- 判断某设备的某个点位是否需要从上送数据中排除
---@param dev_id string
---@param id     integer
---@return boolean
local function is_excluded(dev_id, id)
    if dev_id == "bcu" then
        return BCU_EXCLUDE_IDS[id] == true
    end
    return false
end

--- 读取单个设备的全量数据点（已排除指定点位）
---@param dev_id string
---@return table
local function collect_device(dev_id)
    local list = dc.read_all(dev_id)
    local points = {}
    if list then
        for _, item in ipairs(list) do
            if not is_excluded(dev_id, item.id) then
                points[#points + 1] = {
                    id    = item.id,
                    key   = item.key,
                    value = item.value,
                }
            end
        end
    end
    return points
end

-- 每30秒按设备分别上送一次全量数据，每个设备使用各自的 sn 和 topic
timer.every(30000, function()
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
