-- 关口表 + 计量电表（标准 DL/T645-2007）：两块电表分别接 /dev/ttyXRUSB1、/dev/ttyXRUSB2，
-- 各自注册为一个数据中心设备，只采集现场关注的点位。
--
--   gateway_meter  关口表     表号 012400023482：总有功/无功功率、频率、正/反向有功电能、总有功电能
--   metering_meter 计量电表   表号 000003050215：总有功/无功功率、频率、正/反向总有功电能、
--                                                 正/反向有功电能 尖/峰/平/谷/深谷、总有功电能
--
-- 总有功功率（02030000）、总无功功率（02040000）带符号，原值上送；总有功电能取组合有功总电能（00000000）。
-- 费率电能：费率 1~5 依次为 尖/峰/平/谷/深谷（DI0 = 01~05），实际费率顺序以电表设置为准；
--   电表只有 4 个费率时深谷读不到属正常（可选点位，仅首次失败提示）。
--
-- 变比换算：电表内变比寄存值为 1（未设置），故不再读取，改为在 CONFIG 中按现场铭牌配置 pt / ct，
--   scale = "pt" 的点位（电压）乘 pt，"ct"（电流）乘 ct，"ptct"（功率、电能）乘 pt×ct。
--   关口表 CT 3000/5 = 600；计量电表 CT 300/5 = 60；PT 均按 1（直接接入）。
--
-- 只读：dlt645 API 不支持写，故不提供下发。
-- 启用方式：在 _manifest.lua 的返回列表里加入本脚本文件名，并按现场修改 CONFIG。
-- 注意：CONFIG 中的 dev_id 不能和 modbus_dev / dlt645_dev 已配置的设备重复，串口也不能被其它设备占用。
MOD = {
    name = "关口表和计量电表",
    description = "煜邦储能柜项目_关口表与计量电表（DL/T645-2007）",
}

-- 点位定义（POINTS 中每项）：
--   di       数据标识（DI3 DI2 DI1 DI0）
--   len      数据域字节数（小端压缩 BCD，不含 4 字节 DI）
--   decimals 隐含小数位
--   signed   最高字节 bit7 为符号位
--   optional 部分电表不支持，读取失败属正常，仅首次失败时以 info 提示
local DI_FREQUENCY = { di = 0x02800002, len = 2, decimals = 2 }

local function energy(id, key, name, di, optional)
    return { id = id, key = key, name = name, di = di, len = 4, decimals = 2, unit = "kWh", optional = optional, scale = "ptct" }
end

-- 组合有功总电能（00000000）：正向+反向的代数和，可能为负，按带符号解码
local function total_energy(id, key, name)
    return { id = id, key = key, name = name, di = 0x00000000, len = 4, decimals = 2, signed = true, unit = "kWh", scale = "ptct" }
end

-- 总有功功率（带符号，正为正向、负为反向，不拆分）
local function total_power(id, key, name)
    return { id = id, key = key, name = name, unit = "kW", di = 0x02030000, len = 3, decimals = 4, signed = true, scale = "ptct" }
end

local function reactive_power(id, key, name)
    return { id = id, key = key, name = name, unit = "kvar", di = 0x02040000, len = 3, decimals = 4, signed = true, scale = "ptct" }
end

local function frequency(id, key, name)
    return { id = id, key = key, name = name, unit = "Hz", di = DI_FREQUENCY.di, len = DI_FREQUENCY.len, decimals = DI_FREQUENCY.decimals }
end

local CONFIG = {
    poll_ms = 2000,
    -- 两块表各用一个串口（9600 / 8E1），各自独立连接、独立轮询
    meters = {
        {
            dev_id = "gateway_meter", name = "关口表", address = "012400023482",
            pt = 1, ct = 3000 / 5, -- 电流互感器 3000/5
            link = { serial = "/dev/ttyXRUSB1", baud = 9600, parity = "E", address = "012400023482", timeout = 1200 },
            points = {
                total_power(1, "gateMeterTotalPower", "关口电表总有功功率"),
                reactive_power(2, "gateMeterTotalReactivePwr", "关口电表总无功功率"),
                frequency(3, "gateMeterFrequency", "关口电表频率"),
                energy(4, "gateMeterForwardActiveEnergy", "关口电表正向有功电能", 0x00010000),
                energy(5, "gateMeterReverseActiveEnergy", "关口电表反向有功电能", 0x00020000),
                total_energy(6, "gateMetertotalActiveEnergy", "关口电表总有功电能"),
            },
        },
        {
            dev_id = "metering_meter", name = "计量电表", address = "000003050215",
            pt = 1, ct = 300 / 5, -- 电流互感器 300/5
            link = { serial = "/dev/ttyXRUSB2", baud = 9600, parity = "E", address = "000003050215", timeout = 1200 },
            points = {
                total_power(1, "meterTotalPower", "计量电表总有功功率"),
                reactive_power(2, "meterTotalReactivePwr", "计量电表总无功功率"),
                frequency(3, "meterFrequency", "计量电表频率"),
                energy(4, "meterForwardTotalActiveEnergy", "计量电表正向总有功电能", 0x00010000),
                energy(5, "meterReverseTotalActiveEnergy", "计量电表反向总有功电能", 0x00020000),

                energy(6,  "meterForwardActiveEnergySharp",     "计量电表正向有功电能尖",   0x00010100),
                energy(7,  "meterForwardActiveEnergyPeak",      "计量电表正向有功电能峰",   0x00010200),
                energy(8,  "meterForwardActiveEnergyFlat",      "计量电表正向有功电能平",   0x00010300),
                energy(9,  "meterForwardActiveEnergyValley",    "计量电表正向有功电能谷",   0x00010400),
                energy(10, "meterForwardActiveEnergyDeepValley", "计量电表正向有功电能深谷", 0x00010500, true),

                energy(11, "meterReverseActiveEnergySharp",     "计量电表反向有功电能尖",   0x00020100),
                energy(12, "meterReverseActiveEnergyPeak",      "计量电表反向有功电能峰",   0x00020200),
                energy(13, "meterReverseActiveEnergyFlat",      "计量电表反向有功电能平",   0x00020300),
                energy(14, "meterReverseActiveEnergyValley",    "计量电表反向有功电能谷",   0x00020400),
                energy(15, "meterReverseActiveEnergyDeepValley", "计量电表反向有功电能深谷", 0x00020500, true),
                total_energy(16, "metertotalActiveEnergy", "计量电表总有功电能"),
            },
        },
    },
}

-- 换算系数：pt / ct / pt×ct
for _, m in ipairs(CONFIG.meters) do
    m.factor = { pt = m.pt, ct = m.ct, ptct = m.pt * m.ct }
end

-- 整块表连续失败达到该次数且本轮尚无成功点位时，视为表离线，跳过本轮剩余点位（避免逐点等超时）
local OFFLINE_FAIL_LIMIT = 3

local conns = {} -- dev_id -> 串口连接
local logged_fail = {} -- dev_id .. di -> true：已经告警过的读取失败，恢复后清除

local function ensure_conn(m)
    if conns[m.dev_id] then
        return conns[m.dev_id]
    end
    local c, err = dlt645.connect(m.link)
    if not c then
        log.warn(string.format("[%s] 打开串口 %s 失败: %s", m.dev_id, m.link.serial, tostring(err)))
        return nil
    end
    conns[m.dev_id] = c
    return c
end

local devs = {}
for _, m in ipairs(CONFIG.meters) do
    local dev, derr = dc.register(m.dev_id)
    if not dev then
        log.error("注册设备失败 " .. m.dev_id .. ": " .. tostring(derr))
        return
    end
    devs[m.dev_id] = dev
end

--- 四舍五入到 n 位小数
local function round(v, n)
    local p = 10 ^ (n or 0)
    if v >= 0 then
        return math.floor(v * p + 0.5) / p
    end
    return -math.floor(-v * p + 0.5) / p
end

--- 读取并解码一个 DI，失败返回 nil, err
local function read_di(c, address, def)
    local bytes, err = c:read_raw(def.di, address)
    if not bytes then
        return nil, err
    end
    if #bytes ~= def.len then
        return nil, string.format("数据长度不符: 期望 %d 实际 %d", def.len, #bytes)
    end
    return dlt645.decode(bytes, def.decimals or 0, def.signed or false)
end

local function poll_meter(c, m)
    local dev = devs[m.dev_id]
    local tag = string.format("[%s %s] ", m.dev_id, m.address)
    local points, fails = {}, 0
    local cache = {} -- di -> 本轮已读结果（正/反向功率共用同一个 DI，只读一次）

    for _, def in ipairs(m.points) do
        local fkey = m.dev_id .. string.format("%08X", def.di)
        local cached = cache[def.di]
        local v, err
        if cached then
            v, err = cached.v, cached.err
        else
            v, err = read_di(c, m.address, def)
            cache[def.di] = { v = v, err = err }
            wait(50) -- 相邻请求留出总线空闲时间
        end

        if v then
            logged_fail[fkey] = nil
            fails = 0
            if def.scale then
                v = v * m.factor[def.scale]
            end
            v = round(v, def.decimals) -- 去掉乘变比产生的浮点误差（如 1.7999999999999998）
            points[#points + 1] = {
                id = def.id, key = def.key, name = def.name,
                unit = def.unit, type = "f64", value = v,
            }
        else
            if not cached then
                if not def.optional then
                    fails = fails + 1
                end
                if not logged_fail[fkey] then
                    logged_fail[fkey] = true
                    local msg = string.format("%s读 %s(%08X) 失败: %s", tag, def.key, def.di, tostring(err))
                    if def.optional then
                        log.info(msg .. "（可选点位，电表可能不支持）")
                    else
                        log.warn(msg)
                    end
                end
                if #points == 0 and fails >= OFFLINE_FAIL_LIMIT then
                    log.warn(tag .. "连续读取失败，判定离线，跳过本轮")
                    break
                end
            end
        end
    end

    if #points > 0 then
        dev:ingest(points)
    end
    dev:set_comm(#points > 0)
end

local function poll(m)
    local c = ensure_conn(m)
    if not c then
        devs[m.dev_id]:set_comm(false)
        return
    end
    poll_meter(c, m)
end

for _, m in ipairs(CONFIG.meters) do
    task.spawn(function()
        while true do
            poll(m)
            wait(CONFIG.poll_ms)
        end
    end)
end
