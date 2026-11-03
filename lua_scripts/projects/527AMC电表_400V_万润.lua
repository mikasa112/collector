-- AMC 多功能表（Modbus RTU）：读取 0x0023~0x003E，注册为数据中心设备。
-- 两块表挂在同一串口（RS485 总线）上，从站地址分别为 2、3，共用一个连接，串行轮询。
--
-- 换算（文档 6.5.2）：
--   0x0023 高字节 DPT、低字节 DCT；0x0024 高字节 DPQ、低字节为功率符号位。
--   电压 = 读值 × 10^(DPT-4) V；电流 = 读值 × 10^(DCT-4) A；功率 = 读值 × 10^(DPQ-4) W/var/VA，上报时统一除以 1000 为 kW/kvar/kVA。
--   符号位（高到低）：Q、Qc、Qb、Qa、P、Pc、Pb、Pa，置 1 表示负。视在功率无符号。
--   功率因数 = 读值/1000；频率 = 读值/100 Hz。
MOD = {
    name = "527AMC",
    description = "万润5MWh项目_527AMC",
}

local CONFIG = {
    poll_ms = 3000,
    offline_after = 3, -- 连续读取失败多少次才置通讯故障，滤掉总线偶发超时
    -- 同一串口，从站地址不同；dev_id 全局唯一
    link = { type = "rtu", serial = "/dev/ttyS3", baud = 9600, parity = "N", timeout = 1000 },
    meters = {
        { dev_id = "amc_meter_1", slave = 2 },
        { dev_id = "amc_meter_2", slave = 3 },
    },
}

local BASE_ADDR = 0x0023
local REG_COUNT = 0x003F - 0x0023

-- 点表：kind 决定换算方式（U/I/P 取小数位寄存器，fixed 用 scale）；sign 为功率符号位序号（bit0=Pa ... bit7=Q）
local POINTS = {
    { addr = 0x0025, key = "Ua", name = "A相电压", unit = "V", kind = "U" },
    { addr = 0x0026, key = "Ub", name = "B相电压", unit = "V", kind = "U" },
    { addr = 0x0027, key = "Uc", name = "C相电压", unit = "V", kind = "U" },
    { addr = 0x0028, key = "Uab", name = "AB线电压", unit = "V", kind = "U" },
    { addr = 0x0029, key = "Ubc", name = "BC线电压", unit = "V", kind = "U" },
    { addr = 0x002A, key = "Uac", name = "AC线电压", unit = "V", kind = "U" },

    { addr = 0x002B, key = "Ia", name = "A相电流", unit = "A", kind = "I" },
    { addr = 0x002C, key = "Ib", name = "B相电流", unit = "A", kind = "I" },
    { addr = 0x002D, key = "Ic", name = "C相电流", unit = "A", kind = "I" },

    { addr = 0x002E, key = "Pa", name = "A相有功功率", unit = "kW", kind = "P", sign = 0 },
    { addr = 0x002F, key = "Pb", name = "B相有功功率", unit = "kW", kind = "P", sign = 1 },
    { addr = 0x0030, key = "Pc", name = "C相有功功率", unit = "kW", kind = "P", sign = 2 },
    { addr = 0x0031, key = "Ps", name = "总有功功率", unit = "kW", kind = "P", sign = 3 },

    { addr = 0x0032, key = "Qa", name = "A相无功功率", unit = "kvar", kind = "P", sign = 4 },
    { addr = 0x0033, key = "Qb", name = "B相无功功率", unit = "kvar", kind = "P", sign = 5 },
    { addr = 0x0034, key = "Qc", name = "C相无功功率", unit = "kvar", kind = "P", sign = 6 },
    { addr = 0x0035, key = "Qs", name = "总无功功率", unit = "kvar", kind = "P", sign = 7 },

    { addr = 0x0036, key = "PFa", name = "A相功率因数", kind = "fixed", scale = 0.001, decimals = 3 },
    { addr = 0x0037, key = "PFb", name = "B相功率因数", kind = "fixed", scale = 0.001, decimals = 3 },
    { addr = 0x0038, key = "PFc", name = "C相功率因数", kind = "fixed", scale = 0.001, decimals = 3 },
    { addr = 0x0039, key = "PFs", name = "总功率因数", kind = "fixed", scale = 0.001, decimals = 3 },

    { addr = 0x003A, key = "Sa", name = "A相视在功率", unit = "kVA", kind = "P" },
    { addr = 0x003B, key = "Sb", name = "B相视在功率", unit = "kVA", kind = "P" },
    { addr = 0x003C, key = "Sc", name = "C相视在功率", unit = "kVA", kind = "P" },
    { addr = 0x003D, key = "Ss", name = "总视在功率", unit = "kVA", kind = "P" },

    { addr = 0x003E, key = "F", name = "频率", unit = "Hz", kind = "fixed", scale = 0.01, decimals = 2 },
}

for i, def in ipairs(POINTS) do
    def.id = i
end

local fail_count = {} -- dev_id -> 连续失败次数

local conn -- 共用连接；modbus.connect 失败时为 nil，轮询里会重试

local devs = {}
for _, m in ipairs(CONFIG.meters) do
    local dev, derr = dc.register(m.dev_id)
    if dev then
        devs[#devs + 1] = { m = m, dev = dev }
    else
        log.error("注册设备失败: " .. tostring(derr))
    end
end

-- 按小数位四舍五入，消除 raw*10^n 的浮点误差（如 235.20000000000002）
local function round(v, decimals)
    return tonumber(string.format("%." .. decimals .. "f", v))
end

-- 10^(d-4) 对应的小数位数：d>=4 时为整数
local function decimals_of(d)
    return d < 4 and (4 - d) or 0
end

local function poll_meter(m, dev)
    local tag = "[" .. m.dev_id .. "] "
    local regs, err = conn:read_holding(BASE_ADDR, REG_COUNT, m.slave)
    -- 引擎对脚本单次执行只给 200ms 预算（wait 让出时重置），而串口读取的等待时间也计入其中；
    -- 读完立刻 wait(1) 让出一次重置预算，否则慢读取/重试后继续跑 Lua 会被判超时，协程被杀、数据停更
    wait(1)
    if not regs then
        local n = (fail_count[m.dev_id] or 0) + 1
        fail_count[m.dev_id] = n
        log.warn(tag .. "读取失败(连续" .. n .. "次): " .. tostring(err))
        if n >= CONFIG.offline_after then
            dev:set_comm(false)
        end
        return
    end
    fail_count[m.dev_id] = 0

    local r23, r24 = regs[1], regs[2]
    local dpt = math.floor(r23 / 256)
    local dct = r23 % 256
    local dpq = math.floor(r24 / 256)
    local sign_bits = r24 % 256
    local k_u = 10 ^ (dpt - 4)
    local k_i = 10 ^ (dct - 4)
    local k_p = 10 ^ (dpq - 4)
    local dec_u, dec_i, dec_p = decimals_of(dpt), decimals_of(dct), decimals_of(dpq)

    local out = {}
    for _, def in ipairs(POINTS) do
        local raw = modbus.decode(regs, "U16", nil, def.addr - BASE_ADDR + 1) -- index 为 1 基
        local v
        if def.kind == "U" then
            v = round(raw * k_u, dec_u)
        elseif def.kind == "I" then
            v = round(raw * k_i, dec_i)
        elseif def.kind == "P" then
            v = round(raw * k_p / 1000, dec_p + 3) -- W -> kW
            if def.sign and math.floor(sign_bits / 2 ^ def.sign) % 2 == 1 then
                v = -v
            end
        else
            v = round(raw * def.scale, def.decimals)
        end
        out[#out + 1] = {
            id = def.id, key = def.key, name = def.name, unit = def.unit,
            type = "f64", value = v,
        }
    end
    dev:ingest(out)
    dev:set_comm(true)
end

if #devs > 0 then
    task.spawn(function()
        while true do
            if not conn then
                local c, err = modbus.connect(CONFIG.link)
                wait(1) -- 连接耗时同样计入预算
                if c then
                    conn = c
                else
                    log.warn("连接失败: " .. tostring(err))
                    for _, d in ipairs(devs) do d.dev:set_comm(false) end
                end
            end
            if conn then
                for _, d in ipairs(devs) do
                    poll_meter(d.m, d.dev)
                    wait(200) -- 相邻从站请求留出总线空闲时间
                end
            end
            wait(CONFIG.poll_ms)
        end
    end)
end
