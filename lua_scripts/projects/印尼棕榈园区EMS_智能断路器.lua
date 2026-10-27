MOD = {
    name = "智能断路器",
    description = "印尼棕榈园区EMS_智能断路器",
}

local CONFIG = {
    dev_id = "breaker",
    poll_ms = 5000,
    link = { serial = "/dev/ttyUSB3", baud = 9600, parity = "E", address = "000000000001", timeout = 1200 },
    -- 点位：di 为数据标识；内置 DI 表已收录的无需 spec，未收录的需给 { len, decimals, signed }
    points = {
        { id = 1, key = "energy",  name = "正向有功总电能", di = 0x00010000 },
        { id = 2, key = "voltage", name = "A相电压",        di = 0x02010100 },
        { id = 3, key = "current", name = "A相电流",        di = 0x02020100 },
        { id = 4, key = "power",   name = "总有功功率",      di = 0x02030000 },
        { id = 5, key = "freq",    name = "电网频率",        di = 0x02800002 },
    },
}

local conn

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

local function poll()
    local c = ensure_conn()
    if not c then
        dev:set_comm(false)
        return
    end
end

task.spawn(function ()
    while true do

    end
end)
