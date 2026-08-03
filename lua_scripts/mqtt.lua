MOD = {
    name        = "云端MQTT规则",
    description = "英博云平台MQTT上送/下行",
}

-- host: "47.98.106.76"
-- port: 1883

local HOST = "47.98.106.76"
local PORT = 1883
-- local SN = "HGTMPRSXKLMW"
local SN = "BVXZRNQHLMTP"
local TOPIC = "/gateway/prod/v1/" .. SN .. "/up"
local TOPIC_DOWN = "/gateway/prod/v1/" .. SN .. "/down"

local conn = nil

--- 处理云端下行控制消息
--- payload 结构: { dev = "pcs", data = { {id=1, value=100}, ... } }
local function handle_downlink(_topic, payload)
    local ok, msg = pcall(json.decode, payload)
    if not ok or type(msg) ~= "table" then
        log.warn("mqtt 下行消息解析失败: " .. tostring(msg))
        return
    end
    if type(msg.dev) ~= "string" or type(msg.data) ~= "table" then
        log.warn("mqtt 下行消息格式错误: " .. tostring(payload))
        return
    end
    for _, item in ipairs(msg.data) do
        local ok2, err = pcall(dc.dispatch, msg.dev, item.id, item.value)
        if not ok2 then
            log.warn("mqtt 下行下发失败 dev=" .. msg.dev .. " id=" .. tostring(item.id) .. ": " .. tostring(err))
        end
    end
end

--- 建立MQTT连接，断开/失败后自动重试
local function connect_mqtt()
    while true do
        local c, err = mqtt.connect({
            host = HOST,
            port = PORT,
            max_packet_size = 1024 * 256
        })
        if c then
            conn = c
            log.info("mqtt 连接成功: " .. HOST .. ":" .. PORT)
            local ok, sub_err = pcall(function()
                conn:subscribe(TOPIC_DOWN, handle_downlink)
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
    if dev_id == "emu" then
        return id >= 500 and id <= 2000
    end
    return false
end

--- 按设备汇总全量数据点，结构为 { [dev_id] = { {id=, key=, value=}, ... }, ... }
local function collect_all()
    local data = {}
    for _, dev_id in ipairs(dc.dev_ids()) do
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
        data[dev_id] = points
    end
    return data
end

-- 每30秒上送一次所有设备全量数据
timer.every(30000, function()
    if not conn then
        log.warn("mqtt 未连接，跳过本次上送")
        return
    end
    local payload = {
        sn        = SN,
        timestamp = os.time(),
        data      = collect_all(),
    }
    local ok, err = pcall(function()
        conn:publish(TOPIC, payload)
    end)
    if not ok then
        log.warn("mqtt 上送失败: " .. tostring(err))
    end
end)
