-- AMC 多功能表（Modbus RTU）：读取 0x0023~0x0042（含吸收/释放有功电能），另读 PT/CT 变比，注册为数据中心设备。
-- 四块表挂在同一串口（RS485 总线）上，从站地址分别为 2、3、4、5，共用一个连接，串行轮询。
--
-- 换算（文档 6.5.2）：
--   0x0023 高字节 DPT、低字节 DCT；0x0024 高字节 DPQ、低字节为功率符号位。
--   电压 = 读值 × 10^(DPT-4) V；电流 = 读值 × 10^(DCT-4) A；功率 = 读值 × 10^(DPQ-4) W/var/VA，上报时统一除以 1000 为 kW/kvar/kVA。
--   符号位（高到低）：Q、Qc、Qb、Qa、P、Pc、Pb、Pa，置 1 表示负。视在功率无符号。
--   功率因数 = 读值/1000；频率 = 读值/100 Hz。
--   吸收有功电能（0x003F~0x0040）、释放有功电能（0x0041~0x0042），均为二次侧：Val_t = 高字×65536 + 低字；一次侧 = Val_t/1000 × PT × CT，单位 kWh。
--   PT 在 0x0003、CT 在 0x0004，变化极少，读到后缓存，每隔 RATIO_REFRESH_POLLS 次成功采集刷新一次。
MOD = {
    name = "527AMC",
    description = "万润5MWh项目_527AMC",
}

local CONFIG = {
    tick_ms = 1000, -- 轮询节拍；各表 poll_ms 按节拍取整，未单独指定的表用 poll_ms
    poll_ms = 3000,
    slave_gap_ms = 100, -- 相邻从站请求之间留出的总线空闲时间
    offline_after = 3, -- 连续读取失败多少次才置通讯故障，滤掉总线偶发超时
    -- 同一串口，从站地址不同；dev_id 全局唯一
    link = { type = "rtu", serial = "/dev/ttyS3", baud = 9600, parity = "N", timeout = 1000 },
    meters = {
        { dev_id = "amc_meter_1", slave = 2 },
        { dev_id = "amc_meter_2", slave = 3 },
        { dev_id = "amc_meter_3", slave = 4, poll_ms = 1000 },
        { dev_id = "amc_meter_4", slave = 5, poll_ms = 1000 },
    },
}

local BASE_ADDR = 0x0023
local REG_COUNT = 0x0043 - 0x0023 -- 含 0x003F~0x0042 吸收/释放有功电能
-- 电能点：addr 为二次侧 Uint32 起始地址（高字在前）
local ENERGY_POINTS = {
    { addr = 0x003F, id = 26, key = "EPimp", name = "吸收有功电能" },
    { addr = 0x0041, id = 27, key = "EPexp", name = "释放有功电能" },
}
local RATIO_ADDR = 0x0003 -- 0x0003 PT、0x0004 CT
local RATIO_REFRESH_POLLS = 600

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
local ratios = {} -- dev_id -> { pt =, ct =, age = 距上次读取的成功采集次数 }

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

-- 读取并缓存 PT/CT；读取失败时保留旧缓存，从未读到过则返回 nil（电能点不上报，避免上报错误倍率的值）
local function get_ratio(m)
    local r = ratios[m.dev_id]
    if r and r.age < RATIO_REFRESH_POLLS then
        r.age = r.age + 1
        return r
    end
    wait(CONFIG.slave_gap_ms)
    local regs, err = conn:read_holding(RATIO_ADDR, 2, m.slave)
    wait(1)
    if regs and regs[1] and regs[2] and regs[1] >= 1 and regs[2] >= 1 then
        r = { pt = regs[1], ct = regs[2], age = 0 }
        ratios[m.dev_id] = r
    else
        log.warn("[" .. m.dev_id .. "] 读取PT/CT失败: " .. tostring(err))
    end
    return r
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

    -- 有功电能（一次侧 kWh）
    local ratio = get_ratio(m)
    if ratio then
        for _, e in ipairs(ENERGY_POINTS) do
            local i = e.addr - BASE_ADDR + 1
            local val_t = regs[i] * 65536 + regs[i + 1]
            out[#out + 1] = {
                id = e.id, key = e.key, name = e.name, unit = "kWh",
                type = "f64", value = round(val_t / 1000 * ratio.pt * ratio.ct, 3),
            }
        end
    end
    dev:ingest(out)
    dev:set_comm(true)
end

-- 各表轮询周期按节拍取整；慢表错开相位，避免同一节拍里读所有表而超出节拍时长
local slow_count = 0
for _, d in ipairs(devs) do
    local every = math.max(1, math.ceil((d.m.poll_ms or CONFIG.poll_ms) / CONFIG.tick_ms))
    d.every = every
    if every > 1 then
        d.phase = slow_count % every
        slow_count = slow_count + 1
    end
end

if #devs > 0 then
    local tick = 0
    local busy = false -- 上一节拍还没跑完时跳过本节拍，避免并发占用串口

    local function run_tick()
        if not conn then
            local c, err = modbus.connect(CONFIG.link)
            wait(1) -- 连接耗时同样计入预算
            if c then
                conn = c
            else
                log.warn("连接失败: " .. tostring(err))
                for _, d in ipairs(devs) do d.dev:set_comm(false) end
                return
            end
        end
        for _, d in ipairs(devs) do
            if d.every == 1 or tick % d.every == d.phase then
                poll_meter(d.m, d.dev)
                wait(CONFIG.slave_gap_ms)
            end
        end
    end

    timer.every(CONFIG.tick_ms, function()
        if busy then return end
        busy = true
        tick = tick + 1
        task.spawn(function()
            local ok, err = pcall(run_tick)
            if not ok then
                log.error("轮询异常: " .. tostring(err))
                -- 调用被中断后连接对象可能一直处于"被借用"状态，必须丢弃并回收，下一节拍重新连接
                conn = nil
                collectgarbage()
            end
            busy = false
        end)
    end)
end
