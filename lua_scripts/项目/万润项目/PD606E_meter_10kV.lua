-- PD606 电表 10kV（Modbus RTU）：读取 0x0600~0x065A 长整型电参区，注册为数据中心设备。
-- 两块表挂在同一串口（RS485 总线）上，从站地址分别为 1、2，共用一个连接，串行轮询。
--
-- 每个点占 2 个寄存器（32 位），默认按 ABCD 字序解码；若实测数值异常，改 CONFIG.word_order（如 "CDAB"）。
-- 表内电压/电流/功率及电能 EPP/EPN/EQP/EQN 均为一次侧值，不需要 PT/CT 折算；电能 float（Wh）上报时换算为 kWh。
-- 二次侧电能 WPP/WPN/WQP/WQN（0x0634~0x063B）不采集。
MOD = {
    name = "PD606E",
    description = "万润5MWh项目_PD606_10kV",
}

local CONFIG = {
    poll_ms = 2000,
    word_order = "ABCD",
    offline_after = 3, -- 连续读取失败多少次才置通讯故障，滤掉总线偶发超时
    -- 同一串口，从站地址不同；dev_id 全局唯一
    link = { type = "rtu", serial = "/dev/ttyS0", baud = 9600, parity = "N", timeout = 1000 },
    meters = {
        { dev_id = "pd606_10kv_meter_1", slave = 1 },
        { dev_id = "pd606_10kv_meter_2", slave = 2 },
    },
}

-- 起始地址与寄存器总数：0x0600 ~ 0x065B（最后一个点 0x065A 占 0x065A/0x065B）
local BASE_ADDR = 0x0600
local REG_COUNT = 0x065C - 0x0600
-- 分块读取：引擎对脚本单次执行预算仅 200ms，串口读取的等待也计入；
-- 一次读 92 个寄存器在 9600 波特率下超过 200ms，故拆成小块（须为偶数，保证 32 位点不跨块）
local CHUNK = 24

-- 点表：addr 为协议地址，scale 为转换系数，fmt 默认 I32（Long 有符号）；U32 / F32 见各行
local POINTS = {
    { addr = 0x0600, key = "Ua", name = "A相电压", unit = "V", scale = 0.1 },
    { addr = 0x0602, key = "Ub", name = "B相电压", unit = "V", scale = 0.1 },
    { addr = 0x0604, key = "Uc", name = "C相电压", unit = "V", scale = 0.1 },
    { addr = 0x0606, key = "Uab", name = "AB线电压", unit = "V", scale = 0.1 },
    { addr = 0x0608, key = "Ubc", name = "BC线电压", unit = "V", scale = 0.1 },
    { addr = 0x060A, key = "Uca", name = "CA线电压", unit = "V", scale = 0.1 },

    { addr = 0x060C, key = "Ia", name = "A相电流", unit = "A", scale = 0.001 },
    { addr = 0x060E, key = "Ib", name = "B相电流", unit = "A", scale = 0.001 },
    { addr = 0x0610, key = "Ic", name = "C相电流", unit = "A", scale = 0.001 },

    { addr = 0x0612, key = "Pa", name = "A相有功功率", unit = "kW", scale = 0.001 },
    { addr = 0x0614, key = "Pb", name = "B相有功功率", unit = "kW", scale = 0.001 },
    { addr = 0x0616, key = "Pc", name = "C相有功功率", unit = "kW", scale = 0.001 },
    { addr = 0x0618, key = "Ps", name = "总有功功率", unit = "kW", scale = 0.001 },

    { addr = 0x061A, key = "Qa", name = "A相无功功率", unit = "kvar", scale = 0.001 },
    { addr = 0x061C, key = "Qb", name = "B相无功功率", unit = "kvar", scale = 0.001 },
    { addr = 0x061E, key = "Qc", name = "C相无功功率", unit = "kvar", scale = 0.001 },
    { addr = 0x0620, key = "Qs", name = "总无功功率", unit = "kvar", scale = 0.001 },

    { addr = 0x0622, key = "PFa", name = "A相功率因数", scale = 0.001 },
    { addr = 0x0624, key = "PFb", name = "B相功率因数", scale = 0.001 },
    { addr = 0x0626, key = "PFc", name = "C相功率因数", scale = 0.001 },
    { addr = 0x0628, key = "PFs", name = "总功率因数", scale = 0.001 },

    { addr = 0x062A, key = "Sa", name = "A相视在功率", unit = "kVA", scale = 0.001 },
    { addr = 0x062C, key = "Sb", name = "B相视在功率", unit = "kVA", scale = 0.001 },
    { addr = 0x062E, key = "Sc", name = "C相视在功率", unit = "kVA", scale = 0.001 },
    { addr = 0x0630, key = "Ss", name = "总视在功率", unit = "kVA", scale = 0.001 },

    { addr = 0x0632, key = "F", name = "频率", unit = "Hz", scale = 0.01 },

    -- 一次侧电能（float，Wh -> kWh），即用户实际用电量
    { addr = 0x063C, key = "EPP", name = "正向有功电能", unit = "kWh", scale = 0.001, fmt = "F32" },
    { addr = 0x063E, key = "EPN", name = "反向有功电能", unit = "kWh", scale = 0.001, fmt = "F32" },
    { addr = 0x0640, key = "EQP", name = "正向无功电能", unit = "kvarh", scale = 0.001, fmt = "F32" },
    { addr = 0x0642, key = "EQN", name = "反向无功电能", unit = "kvarh", scale = 0.001, fmt = "F32" },

    { addr = 0x0644, key = "PSVC", name = "正序电压分量", unit = "V", scale = 0.1 },
    { addr = 0x0646, key = "NSVC", name = "负序电压分量", unit = "V", scale = 0.1 },
    { addr = 0x0648, key = "ZSVC", name = "零序电压分量", unit = "V", scale = 0.1 },
    { addr = 0x064A, key = "PSAC", name = "正序电流分量", unit = "A", scale = 0.001 },
    { addr = 0x064C, key = "NSAC", name = "负序电流分量", unit = "A", scale = 0.001 },
    { addr = 0x064E, key = "ZSAC", name = "零序电流分量", unit = "A", scale = 0.001 },

    { addr = 0x0650, key = "THDVa", name = "A相电压总谐波畸变率", scale = 0.001 },
    { addr = 0x0652, key = "THDVb", name = "B相电压总谐波畸变率", scale = 0.001 },
    { addr = 0x0654, key = "THDVc", name = "C相电压总谐波畸变率", scale = 0.001 },
    { addr = 0x0656, key = "THDIa", name = "A相电流总谐波畸变率", scale = 0.001 },
    { addr = 0x0658, key = "THDIb", name = "B相电流总谐波畸变率", scale = 0.001 },
    { addr = 0x065A, key = "THDIc", name = "C相电流总谐波畸变率", scale = 0.001 },
}

for i, def in ipairs(POINTS) do
    def.id = i
end

local fail_count = {} -- dev_id -> 连续失败次数
local conn -- 共用连接；modbus.connect 失败时为 nil，轮询里会重试

local devs = {}
for _, m in ipairs(CONFIG.meters) do
    -- 纯采集设备，无下发点位
    local dev, derr = dc.register(m.dev_id)
    if dev then
        devs[#devs + 1] = { m = m, dev = dev }
    else
        log.error("注册设备失败: " .. tostring(derr))
    end
end

-- 按 scale 对应的小数位四舍五入，消除 raw*scale 的浮点误差
local function round(v, scale)
    local decimals = scale < 1 and math.floor(-math.log(scale) / math.log(10) + 0.5) or 0
    return tonumber(string.format("%." .. decimals .. "f", v))
end

-- 读失败：累计连续失败次数，达到阈值才置通讯故障
local function read_failed(m, dev, what, err)
    local n = (fail_count[m.dev_id] or 0) + 1
    fail_count[m.dev_id] = n
    log.warn("[" .. m.dev_id .. "] " .. what .. "失败(连续" .. n .. "次): " .. tostring(err))
    if n >= CONFIG.offline_after then
        dev:set_comm(false)
    end
end

local function poll_meter(m, dev)
    local regs = {}
    for off = 0, REG_COUNT - 1, CHUNK do
        local n = math.min(CHUNK, REG_COUNT - off)
        local part, err = conn:read_holding(BASE_ADDR + off, n, m.slave)
        -- 读完立刻 wait 让出一次重置预算，否则读取耗时之后继续跑 Lua 会被判超时
        wait(20)
        if not part then
            read_failed(m, dev, string.format("读取0x%04X", BASE_ADDR + off), err)
            return
        end
        for i = 1, n do
            regs[off + i] = part[i]
        end
    end
    fail_count[m.dev_id] = 0

    local out = {}
    for _, def in ipairs(POINTS) do
        local idx = def.addr - BASE_ADDR + 1 -- decode 的 index 为 1 基
        local raw = modbus.decode(regs, def.fmt or "I32", CONFIG.word_order, idx)
        out[#out + 1] = {
            id = def.id, key = def.key, name = def.name, unit = def.unit,
            type = "f64", value = round(raw * def.scale, def.scale),
        }
    end
    dev:ingest(out)
    dev:set_comm(true)
end

if #devs > 0 then
    local function run()
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
    end

    -- 监督：任何错误（含引擎 200ms 预算超时中断）都只记日志并重启轮询，避免协程被杀后数据停更
    task.spawn(function()
        while true do
            local ok, err = pcall(run)
            if not ok then
                log.error("轮询异常，1秒后重启: " .. tostring(err))
                -- 调用被中断后连接对象可能一直处于"被借用"状态，必须丢弃并回收，下一轮重新连接
                conn = nil
                collectgarbage()
            end
            wait(1000)
        end
    end)
end
