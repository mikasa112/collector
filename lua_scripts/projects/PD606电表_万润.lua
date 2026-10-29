-- PD606 电表（Modbus RTU）：读取 0x0600~0x065A 长整型电参区，注册为数据中心设备。
--
-- 每个点占 2 个寄存器（32 位），默认按 ABCD 字序解码；若实测数值异常，改 CONFIG.word_order（如 "CDAB"）。
-- 电能：二次侧 WPP/WPN/WQP/WQN 为 Long（Wh），一次侧 EPP/EPN/EQP/EQN 为 float（Wh），上报时统一换算为 kWh。
MOD = {
    name = "PD606",
    description = "万润5MWh项目_PD606",
}

local CONFIG = {
    poll_ms = 3000,
    word_order = "ABCD",
    -- 每块表一个条目，按一表一串口配置：dev_id 全局唯一，串口不要与其他设备共用
    meters = {
        { dev_id = "pd606_meter_1", link = { type = "rtu", serial = "/dev/ttyS1", baud = 9600, parity = "N", slave = 1, timeout = 1000 } },
        { dev_id = "pd606_meter_2", link = { type = "rtu", serial = "/dev/ttyS2", baud = 9600, parity = "N", slave = 1, timeout = 1000 } },
        { dev_id = "pd606_meter_3", link = { type = "rtu", serial = "/dev/ttyS3", baud = 9600, parity = "N", slave = 1, timeout = 1000 } },
        { dev_id = "pd606_meter_4", link = { type = "rtu", serial = "/dev/ttyS4", baud = 9600, parity = "N", slave = 1, timeout = 1000 } },
        { dev_id = "pd606_meter_5", link = { type = "rtu", serial = "/dev/ttyS5", baud = 9600, parity = "N", slave = 1, timeout = 1000 } },
    },
}

-- 起始地址与寄存器总数：0x0600 ~ 0x065B（最后一个点 0x065A 占 0x065A/0x065B）
local BASE_ADDR = 0x0600
local REG_COUNT = 0x065C - 0x0600

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

    { addr = 0x0612, key = "Pa", name = "A相有功功率", unit = "W", scale = 1 },
    { addr = 0x0614, key = "Pb", name = "B相有功功率", unit = "W", scale = 1 },
    { addr = 0x0616, key = "Pc", name = "C相有功功率", unit = "W", scale = 1 },
    { addr = 0x0618, key = "Ps", name = "总有功功率", unit = "W", scale = 1 },

    { addr = 0x061A, key = "Qa", name = "A相无功功率", unit = "var", scale = 1 },
    { addr = 0x061C, key = "Qb", name = "B相无功功率", unit = "var", scale = 1 },
    { addr = 0x061E, key = "Qc", name = "C相无功功率", unit = "var", scale = 1 },
    { addr = 0x0620, key = "Qs", name = "总无功功率", unit = "var", scale = 1 },

    { addr = 0x0622, key = "PFa", name = "A相功率因数", scale = 0.001 },
    { addr = 0x0624, key = "PFb", name = "B相功率因数", scale = 0.001 },
    { addr = 0x0626, key = "PFc", name = "C相功率因数", scale = 0.001 },
    { addr = 0x0628, key = "PFs", name = "总功率因数", scale = 0.001 },

    { addr = 0x062A, key = "Sa", name = "A相视在功率", unit = "VA", scale = 1 },
    { addr = 0x062C, key = "Sb", name = "B相视在功率", unit = "VA", scale = 1 },
    { addr = 0x062E, key = "Sc", name = "C相视在功率", unit = "VA", scale = 1 },
    { addr = 0x0630, key = "Ss", name = "总视在功率", unit = "VA", scale = 1 },

    { addr = 0x0632, key = "F", name = "频率", unit = "Hz", scale = 0.01 },

    -- 二次侧电能（Long，Wh -> kWh；乘以电压、电流倍率为一次电能）
    { addr = 0x0634, key = "WPP", name = "正向有功电能(二次)", unit = "kWh", scale = 0.001, fmt = "U32" },
    { addr = 0x0636, key = "WPN", name = "反向有功电能(二次)", unit = "kWh", scale = 0.001, fmt = "U32" },
    { addr = 0x0638, key = "WQP", name = "正向无功电能(二次)", unit = "kvarh", scale = 0.001, fmt = "U32" },
    { addr = 0x063A, key = "WQN", name = "反向无功电能(二次)", unit = "kvarh", scale = 0.001, fmt = "U32" },

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

-- 为一块表创建独立的连接与设备，返回其轮询函数；注册失败返回 nil
local function start_meter(m)
    local tag = "[" .. m.dev_id .. "] "
    local conn -- 连接句柄；modbus.connect 失败时为 nil，轮询里会重试

    -- 纯采集设备，无下发点位
    local dev, derr = dc.register(m.dev_id)
    if not dev then
        log.error("注册设备失败: " .. tostring(derr))
        return nil
    end

    return function()
        if not conn then
            local c, err = modbus.connect(m.link)
            if not c then
                log.warn(tag .. "连接失败: " .. tostring(err))
                dev:set_comm(false)
                return
            end
            conn = c
        end
        local regs, err = conn:read_holding(BASE_ADDR, REG_COUNT)
        if not regs then
            log.warn(tag .. "读取失败: " .. tostring(err))
            dev:set_comm(false) -- 传输层错误后连接会自动重建，下一轮继续
            return
        end

        local out = {}
        for _, def in ipairs(POINTS) do
            local idx = def.addr - BASE_ADDR + 1 -- decode 的 index 为 1 基
            local raw = modbus.decode(regs, def.fmt or "I32", CONFIG.word_order, idx)
            out[#out + 1] = {
                id = def.id, key = def.key, name = def.name, unit = def.unit,
                type = "f64", value = raw * def.scale,
            }
        end
        dev:ingest(out)
        dev:set_comm(true)
    end
end

-- 每块表一个独立任务，互不阻塞（某块表超时不会拖慢其他表）
for _, m in ipairs(CONFIG.meters) do
    local poll = start_meter(m)
    if poll then
        task.spawn(function()
            while true do
                poll()
                wait(CONFIG.poll_ms)
            end
        end)
    end
end
