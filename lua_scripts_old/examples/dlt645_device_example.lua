-- 示例：用 Lua 直连一台 DL/T645-2007 电表，并注册为数据中心设备。
-- 适用于点表配置不够灵活的场景（私有 DI、按状态切换读取内容、多表轮询等）。DL/T645 为只读，不处理下发。
--
-- 启用方式：在 _manifest.lua 的返回列表里加入 "dlt645_device_example.lua"，并按实际电表修改 CONFIG。
-- 注意：CONFIG.dev_id 不能与 dlt645_dev 配置的设备重名，串口也不能和它们共用，否则内置设备优先、本脚本注册/连接失败。
MOD = {
    name = "DL/T645 脚本设备示例",
    description = "Lua 直连 DL/T645-2007 电表：周期读取并上报到数据中心",
}

local CONFIG = {
    dev_id = "lua_dlt645",
    poll_ms = 5000,
    link = { serial = "/dev/ttyS1", baud = 2400, parity = "E", address = "000000000001", timeout = 1200 },
    -- 点位：di 为数据标识；内置 DI 表已收录的无需 spec，未收录的需给 { len, decimals, signed }
    points = {
        { id = 1, key = "energy",  name = "正向有功总电能", di = 0x00010000 },
        { id = 2, key = "voltage", name = "A相电压",        di = 0x02010100 },
        { id = 3, key = "current", name = "A相电流",        di = 0x02020100 },
        { id = 4, key = "power",   name = "总有功功率",      di = 0x02030000 },
        { id = 5, key = "freq",    name = "电网频率",        di = 0x02800002 },
    },
}

local conn -- 连接句柄；dlt645.connect 失败时为 nil，轮询里会重试

local function ensure_conn()
    if conn then
        return conn
    end
    local c, err = dlt645.connect(CONFIG.link)
    if not c then
        log.warn("[" .. CONFIG.dev_id .. "] 连接失败: " .. tostring(err))
        return nil
    end
    conn = c
    return conn
end

local dev, derr = dc.register(CONFIG.dev_id)
if not dev then
    log.error("注册设备失败: " .. tostring(derr))
    return
end

-- 一次轮询：逐个 DI 读取。任意一个读成功即认为通讯正常；全部失败才置通讯故障
local function poll()
    local c = ensure_conn()
    if not c then
        dev:set_comm(false)
        return
    end
    local points = {}
    for _, def in ipairs(CONFIG.points) do
        local v, err = c:read(def.di, nil, def.spec)
        if v then
            local info = dlt645.spec(def.di)
            points[#points + 1] = {
                id = def.id, key = def.key, name = def.name,
                unit = info and info.unit ~= "" and info.unit or nil,
                type = "f64", value = v,
            }
        else
            log.warn(string.format("[%s] 读 %s(%08X) 失败: %s", CONFIG.dev_id, def.key, def.di, tostring(err)))
        end
        wait(50) -- 相邻请求留出总线空闲时间
    end
    if #points > 0 then
        dev:ingest(points)
    end
    dev:set_comm(#points > 0)
end

task.spawn(function()
    while true do
        poll()
        wait(CONFIG.poll_ms)
    end
end)
