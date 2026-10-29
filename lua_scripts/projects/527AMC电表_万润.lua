-- AMC 多功能表（DL/T645-2007）：逐个 DI 读取电能/电压/电流/功率/功率因数/频率/变比，注册为数据中心设备。
-- 只读，不处理下发。数据块 DI（00FF0000、0201FF00 等）不使用，改读单点 DI，便于单点失败时不影响其他点。
MOD = {
    name = "527AMC",
    description = "万润5MWh项目_527AMC",
}

local CONFIG = {
    poll_ms = 5000,
    -- 万润现场只有1块400V表。dev_id 全局唯一；address 为表地址（12位十进制）。
    meters = {
        { dev_id = "amc_meter_1", link = { serial = "/dev/ttyS1", baud = 2400, parity = "E", address = "000000000001", timeout = 1200 } },
    },
}

-- 点表：spec 按文档数据格式给出（len 字节数，decimals 小数位，signed 是否带符号位）
local U_V   = { len = 2, decimals = 1 }               -- XXX.X
local I_A   = { len = 3, decimals = 3, signed = true } -- XXX.XXX
local P_KW  = { len = 3, decimals = 4, signed = true } -- XX.XXXX
local PF    = { len = 2, decimals = 3, signed = true } -- X.XXX
local E_KWH = { len = 4, decimals = 2 }               -- XXXXXX.XX

local POINTS = {
    { di = 0x00000000, key = "Ecomb", name = "组合有功总电能", unit = "kWh", spec = { len = 4, decimals = 2, signed = true } },
    { di = 0x00010000, key = "EPP", name = "正向有功总电能", unit = "kWh", spec = E_KWH },
    { di = 0x00020000, key = "EPN", name = "反向有功总电能", unit = "kWh", spec = E_KWH },
    { di = 0x00030000, key = "EQ1", name = "无功1总电能", unit = "kvarh", spec = E_KWH },
    { di = 0x00040000, key = "EQ2", name = "无功2总电能", unit = "kvarh", spec = E_KWH },

    { di = 0x02010100, key = "Ua", name = "A相电压", unit = "V", spec = U_V },
    { di = 0x02010200, key = "Ub", name = "B相电压", unit = "V", spec = U_V },
    { di = 0x02010300, key = "Uc", name = "C相电压", unit = "V", spec = U_V },

    { di = 0x02020100, key = "Ia", name = "A相电流", unit = "A", spec = I_A },
    { di = 0x02020200, key = "Ib", name = "B相电流", unit = "A", spec = I_A },
    { di = 0x02020300, key = "Ic", name = "C相电流", unit = "A", spec = I_A },

    { di = 0x02030000, key = "Ps", name = "总有功功率", unit = "kW", spec = P_KW },
    { di = 0x02030100, key = "Pa", name = "A相有功功率", unit = "kW", spec = P_KW },
    { di = 0x02030200, key = "Pb", name = "B相有功功率", unit = "kW", spec = P_KW },
    { di = 0x02030300, key = "Pc", name = "C相有功功率", unit = "kW", spec = P_KW },

    { di = 0x02040000, key = "Qs", name = "总无功功率", unit = "kvar", spec = P_KW },
    { di = 0x02040100, key = "Qa", name = "A相无功功率", unit = "kvar", spec = P_KW },
    { di = 0x02040200, key = "Qb", name = "B相无功功率", unit = "kvar", spec = P_KW },
    { di = 0x02040300, key = "Qc", name = "C相无功功率", unit = "kvar", spec = P_KW },

    { di = 0x02050000, key = "Ss", name = "总视在功率", unit = "kVA", spec = P_KW },
    { di = 0x02050100, key = "Sa", name = "A相视在功率", unit = "kVA", spec = P_KW },
    { di = 0x02050200, key = "Sb", name = "B相视在功率", unit = "kVA", spec = P_KW },
    { di = 0x02050300, key = "Sc", name = "C相视在功率", unit = "kVA", spec = P_KW },

    { di = 0x02060000, key = "PFs", name = "总功率因数", spec = PF },
    { di = 0x02060100, key = "PFa", name = "A相功率因数", spec = PF },
    { di = 0x02060200, key = "PFb", name = "B相功率因数", spec = PF },
    { di = 0x02060300, key = "PFc", name = "C相功率因数", spec = PF },

    { di = 0x02800002, key = "F", name = "电网频率", unit = "Hz", spec = { len = 2, decimals = 2 } },

    { di = 0x04808080, key = "UT", name = "电压变比", spec = { len = 2, decimals = 0 } },
    { di = 0x04808081, key = "CT", name = "电流变比", spec = { len = 2, decimals = 0 } },
}

for i, def in ipairs(POINTS) do
    def.id = i
end

-- 为一块表创建独立的连接与设备，返回其轮询函数；注册失败返回 nil
local function start_meter(m)
    local tag = "[" .. m.dev_id .. "] "
    local conn -- 连接句柄；dlt645.connect 失败时为 nil，轮询里会重试

    local dev, derr = dc.register(m.dev_id)
    if not dev then
        log.error("注册设备失败: " .. tostring(derr))
        return nil
    end

    -- 逐个 DI 读取。任意一个读成功即认为通讯正常；全部失败才置通讯故障
    return function()
        if not conn then
            local c, err = dlt645.connect(m.link)
            if not c then
                log.warn(tag .. "连接失败: " .. tostring(err))
                dev:set_comm(false)
                return
            end
            conn = c
        end

        local points = {}
        for _, def in ipairs(POINTS) do
            local v, err = conn:read(def.di, nil, def.spec)
            if v then
                points[#points + 1] = {
                    id = def.id, key = def.key, name = def.name, unit = def.unit,
                    type = "f64", value = v,
                }
            else
                log.warn(string.format("%s读 %s(%08X) 失败: %s", tag, def.key, def.di, tostring(err)))
            end
            wait(50) -- 相邻请求留出总线空闲时间
        end
        if #points > 0 then
            dev:ingest(points)
        end
        dev:set_comm(#points > 0)
    end
end

-- 每块表一个独立任务，互不阻塞
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
