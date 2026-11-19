-- 2LC 重合闸剩余电流动作保护器（DL/T —20 剩余电流动作保护器通信规约）
--
-- 规约帧格式与 DL/T645-2007 一致（68H + 6 字节 BCD 地址 + 68H + C + L + DATA(+33H) + CS + 16H，
-- 读数据 C=11H / 应答 91H，异常 D1H），因此直接复用 dlt645 API 的 read_raw 读取规约自定义的 DI。
-- 规约 DI 表见 附录 A（A.2.1 当前变量 / A.2.2 累计记录 / A.2.5 参数变量）。
--
-- 限制：dlt645 API 只读，规约里的遥控跳闸/合闸（C=1CH，需密码）、告警查询（C=19H）暂不支持。
MOD = {
    name = "智能断路器",
    description = "印尼棕榈园区EMS_智能断路器（2LC 重合闸剩余电流动作保护器）",
}

local CONFIG = {
    dev_id = "breaker",
    poll_ms = 5000,
    -- 规约默认 2400bps / 8E1，现场按设备实际设置；address 为 12 位十进制通信地址
    link = { serial = "/dev/ttyS0", baud = 2400, parity = "E", address = "000000000001", timeout = 1200 },
}

-- 点位定义：
--   di       数据标识（DI3 DI2 DI1 DI0）
--   len      数据域字节数；bin=true 表示 BIN 码（小端无符号），否则为小端压缩 BCD
--   decimals 隐含小数位（仅 BCD）
--   optional 增强/扩展功能，设备不支持时读取失败属正常，只在首次失败时告警
local POINTS = {
    -- A.2.1 当前变量（A.1.5：单极/单相保护器 B、C 相无效，读不到属正常）
    { id = 1,  key = "voltageA",              name = "A相电压",             di = 0x02010100, len = 2, decimals = 1, unit = "V" },
    { id = 2,  key = "voltageB",              name = "B相电压",             di = 0x02010200, len = 2, decimals = 1, unit = "V", optional = true },
    { id = 3,  key = "voltageC",              name = "C相电压",             di = 0x02010300, len = 2, decimals = 1, unit = "V", optional = true },
    { id = 4,  key = "currentA",              name = "A相电流",             di = 0x02020100, len = 3, decimals = 1, unit = "A" },
    { id = 5,  key = "currentB",              name = "B相电流",             di = 0x02020200, len = 3, decimals = 1, unit = "A", optional = true },
    { id = 6,  key = "currentC",              name = "C相电流",             di = 0x02020300, len = 3, decimals = 1, unit = "A", optional = true },
    { id = 7,  key = "residualMaxPhase",      name = "剩余电流最大相",       di = 0x02900000, len = 1, bin = true },
    { id = 8,  key = "residualCurrent",       name = "当前剩余电流",         di = 0x02900100, len = 2, unit = "mA" },
    { id = 9,  key = "ratedResidualTripCur",  name = "额定剩余电流动作值",    di = 0x02910100, len = 2, unit = "mA" },
    { id = 10, key = "ratedNonDriveTime",     name = "额定极限不驱动时间",    di = 0x02910200, len = 2, unit = "ms" },

    -- A.2.2 累计记录
    { id = 11, key = "clearCount",            name = "数据清零总次数",       di = 0x03300100, len = 3, unit = "次" },
    { id = 12, key = "tripCountTotal",        name = "总跳闸次数",           di = 0x03810000, len = 2, unit = "次" },
    { id = 13, key = "tripCountLockout",      name = "闭锁性跳闸次数",       di = 0x03810001, len = 2, unit = "次", optional = true },
    { id = 14, key = "tripCountResidual",     name = "剩余电流保护跳闸次数",  di = 0x03810002, len = 2, unit = "次", optional = true },
    { id = 15, key = "tripCountCurrent",      name = "电流保护跳闸次数",      di = 0x03810003, len = 2, unit = "次", optional = true },
    { id = 16, key = "tripCountVoltage",      name = "电压保护跳闸次数",      di = 0x03810004, len = 2, unit = "次", optional = true },
    { id = 17, key = "tripCountManualLock",   name = "手动闭锁跳闸次数",      di = 0x03810005, len = 2, unit = "次", optional = true },
    { id = 18, key = "tripCountNoNeutral",    name = "缺零保护跳闸次数",      di = 0x03810006, len = 2, unit = "次", optional = true },
    { id = 19, key = "tripCountTest",         name = "试验跳闸次数",          di = 0x03810007, len = 2, unit = "次", optional = true },
    { id = 20, key = "residualQuitCount",     name = "退出剩余电流保护次数",   di = 0x03810101, len = 2, unit = "次" },
    { id = 21, key = "runMinutes",            name = "保护器运行时间累计",    di = 0x03810201, len = 4, unit = "分" },

    -- A.2.5 参数变量：运行状态字（原始值 + 下面按 B.1 拆出的位域点）
    { id = 22, key = "runStatus1",            name = "运行状态字1",          di = 0x04000501, len = 1, bin = true },
    { id = 23, key = "runStatus2",            name = "运行状态字2",          di = 0x04000502, len = 1, bin = true },
}

local ID_STATUS1      = 22 -- 运行状态字1 原始值（POINTS 中的 runStatus1）
local ID_STATUS2      = 23 -- 运行状态字2 原始值（POINTS 中的 runStatus2）
local ID_ALARM_FLAG   = 24 -- 状态字1 Bit7：告警状态（0=无告警 1=有告警）
local ID_BREAKER_POS  = 25 -- 状态字1 Bit6~5：闸位（0=合闸 1=保留 2=重合 3=闭锁跳闸）
local ID_FAULT_REASON = 26 -- 状态字1 Bit4~0：跳闸/告警原因（B.1.1，0=正常运行）
local ID_TRIP_POINTER = 27 -- 状态字2 Bit2~0：跳闸次数指针

local conn
local logged_fail = {} -- di -> true：已经告警过的读取失败，恢复后清除

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

--- 读取并解码一个点位，失败返回 nil, err
local function read_point(c, def)
    local bytes, err = c:read_raw(def.di)
    if not bytes then
        return nil, err
    end
    if #bytes ~= def.len then
        return nil, string.format("数据长度不符: 期望 %d 实际 %d", def.len, #bytes)
    end
    if def.bin then
        local v = 0
        for i = #bytes, 1, -1 do
            v = v * 256 + bytes[i]
        end
        return v
    end
    return dlt645.decode(bytes, def.decimals or 0)
end

local function poll()
    local c = ensure_conn()
    if not c then
        dev:set_comm(false)
        return
    end

    local points = {}
    local values = {}
    for _, def in ipairs(POINTS) do
        local v, err = read_point(c, def)
        if v then
            logged_fail[def.di] = nil
            values[def.id] = v
            points[#points + 1] = {
                id = def.id, key = def.key, name = def.name,
                unit = def.unit, type = "f64", value = v,
            }
        elseif not logged_fail[def.di] then
            logged_fail[def.di] = true
            local msg = string.format("[%s] 读 %s(%08X) 失败: %s", CONFIG.dev_id, def.key, def.di, tostring(err))
            if def.optional then
                log.info(msg .. "（可选点位，设备可能不支持）")
            else
                log.warn(msg)
            end
        end
        wait(50) -- 相邻请求留出总线空闲时间（规约要求从站应答延时 20~500ms）
    end

    -- 状态字位域拆分（B.1.1 / B.1.2）
    local s1 = values[ID_STATUS1]
    if s1 then
        points[#points + 1] = { id = ID_ALARM_FLAG,   key = "alarmFlag",   name = "告警状态",       type = "u8", value = (s1 >> 7) & 0x01 }
        points[#points + 1] = { id = ID_BREAKER_POS,  key = "breakerPos",  name = "闸位状态",       type = "u8", value = (s1 >> 5) & 0x03 }
        points[#points + 1] = { id = ID_FAULT_REASON, key = "faultReason", name = "跳闸/告警原因",   type = "u8", value = s1 & 0x1F }
    end
    local s2 = values[ID_STATUS2]
    if s2 then
        points[#points + 1] = { id = ID_TRIP_POINTER, key = "tripPointer", name = "跳闸次数指针",   type = "u8", value = s2 & 0x07 }
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
