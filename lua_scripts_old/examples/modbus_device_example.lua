-- 示例：用 Lua 直连一台寄存器很少的 Modbus 设备，并注册为数据中心设备。
-- 适用于点表配置不够灵活的场景（私有换算、先写后读的握手、按状态切换读取内容等）。
--
-- 启用方式：在 _manifest.lua 的返回列表里加入 "modbus_device_example.lua"，并按实际设备修改 CONFIG。
-- 注意：CONFIG.dev_id 不能与 modbus_dev 配置的设备重名，串口也不能和它们共用，否则内置设备优先、本脚本注册/连接失败。
MOD = {
    name = "Modbus 脚本设备示例",
    description = "Lua 直连 Modbus 设备：周期读取并上报到数据中心，接收 dc.dispatch 下发",
}

local CONFIG = {
    dev_id = "lua_meter",
    poll_ms = 1000,
    -- RTU 示例；改 TCP 则用 { type = "tcp", host = "192.168.1.10", port = 502, slave = 1 }
    link = { type = "rtu", serial = "/dev/ttyS1", baud = 9600, parity = "N", slave = 1, timeout = 800 },
}

-- 点位下发：key -> 目标寄存器。on_write 收到的 points 带的是下发时使用的 id/key/name
local WRITABLE = {
    power_limit = 100, -- 功率限值，U16，单位 0.1kW
}

local conn -- 连接句柄；modbus.connect 失败时为 nil，轮询里会重试

local function ensure_conn()
    if conn then
        return conn
    end
    local c, err = modbus.connect(CONFIG.link)
    if not c then
        log.warn("[" .. CONFIG.dev_id .. "] 连接失败: " .. tostring(err))
        return nil
    end
    conn = c
    return conn
end

local dev, derr = dc.register(CONFIG.dev_id, function(points)
    local c = ensure_conn()
    if not c then
        log.warn("[" .. CONFIG.dev_id .. "] 未连接，丢弃下发")
        return
    end
    for _, p in ipairs(points) do
        local addr = p.key and WRITABLE[p.key]
        if addr then
            -- 读写方法失败返回 nil, err，不会抛异常
            local ok, err = c:write_register(addr, math.floor(p.value))
            if not ok then
                log.warn("[" .. CONFIG.dev_id .. "] 写 " .. p.key .. " 失败: " .. tostring(err))
            end
        else
            log.warn("[" .. CONFIG.dev_id .. "] 不支持下发的点位: " .. tostring(p.key or p.id or p.name))
        end
    end
end)

if not dev then
    log.error("注册设备失败: " .. tostring(derr))
    return
end

-- 一次轮询：读 5 个保持寄存器，换算后上报
-- 0-1: 累计电量 U32(CDAB, 0.01kWh)  2: 温度 I16(0.1℃)  3: 有功功率 I16(0.1kW)  4: 状态字 U16
local function poll()
    local c = ensure_conn()
    if not c then
        dev:set_comm(false)
        return
    end
    local regs, err = c:read_holding(0, 5)
    if not regs then
        log.warn("[" .. CONFIG.dev_id .. "] 读取失败: " .. tostring(err))
        dev:set_comm(false) -- 传输层错误后连接会自动重建，下一轮继续
        return
    end
    dev:ingest({
        { id = 1, key = "energy", name = "累计电量", unit = "kWh", type = "f64",
          value = modbus.decode(regs, "U32", "CDAB") * 0.01 },
        { id = 2, key = "temp", name = "温度", unit = "℃", type = "f64",
          value = modbus.decode(regs, "I16", nil, 3) * 0.1 },
        { id = 3, key = "power", name = "有功功率", unit = "kW", type = "f64",
          value = modbus.decode(regs, "I16", nil, 4) * 0.1 },
        { id = 4, key = "status", name = "状态字", type = "u16", value = regs[5] },
    })
    dev:set_comm(true)
end

task.spawn(function()
    while true do
        poll()
        wait(CONFIG.poll_ms)
    end
end)
