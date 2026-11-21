-- 2LC 重合闸剩余电流动作保护器（DL/T —20 剩余电流动作保护器通信规约）
--
-- 规约帧格式与 DL/T645-2007 一致（68H + 6 字节 BCD 地址 + 68H + C + L + DATA(+33H) + CS + 16H，
-- 读数据 C=11H / 应答 91H，异常 D1H），因此直接复用 dlt645 API 的 read_raw 读取规约自定义的 DI。
-- 规约 DI 表见 附录 A（A.2.1 当前变量 / A.2.2 累计记录 / A.2.5 参数变量）。
--
-- 下发（MQTT yk/yt → dc.dispatch → on_write）：
--   remoteTrip  (id 28) 值为 1 → 立即远程跳闸
--   remoteClose (id 29) 值为 1 → 立即远程合闸
--   其他值（含 0）一律忽略，避免误操作。
--   实现：dlt645 conn:request(0x1C, ...)，数据域 = DI(4) + PA P0 P1 P2(密码) + C0~C3(操作者代码) + DATA，
--   DI = 06010101（预约远程跳闸）/ 06010201（预约远程合闸），DATA = NN uu（NN=0 立即执行）。
--   正常应答 9CH（L=0），异常应答 DCH + ERR（B.3.2：bit3 远程控制失败 bit2 密码错/未授权 bit1 无请求数据项 bit0 数据非法）。
-- 限制：告警查询（C=19H）暂不支持。
-- 注意：MQTT topic 为 /asw/{前缀}/{序号}/yk 时，设备 ID = 前缀 + (序号+1)，所以 dev_id 必须形如 breaker1。
MOD = {
    name = "智能断路器",
    description = "印尼棕榈园区EMS_智能断路器（2LC 重合闸剩余电流动作保护器）",
}

local CONFIG = {
    dev_id = "breaker1", -- MQTT /asw/breaker/0/yk → breaker1
    -- 遥控授权。规约未给默认值，必须按现场设备（04000C01~03 的 0~2 级密码）配置后再联调：
    --   pa = 密码权限 00~02（00 最高）；pw = P0 P1 P2 三个字节（线上顺序）；operator = C0~C3 操作者代码
    control = {
        pa = 0x00,
        pw = { 0x00, 0x00, 0x00 },
        operator = { 0x00, 0x00, 0x00, 0x00 },
        -- DATA(NN uu) 线上字节顺序：规约按小端传输，默认 uu 在前 NN 在后；联调时若设备拒绝可改为 "nn_first"
        data_order = "uu_first",
        unit = 0x02, -- uu：02=分 03=小时（NN=0 立即执行时仅占位）
    },
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

-- 可下发的控制点（值必须为 1 才执行）
local CONTROLS = {
    remoteTrip  = { id = 28, name = "远程跳闸", di = { 0x01, 0x01, 0x01, 0x06 } }, -- DI 06010101，线上 DI0..DI3
    remoteClose = { id = 29, name = "远程合闸", di = { 0x01, 0x02, 0x01, 0x06 } }, -- DI 06010201
}
local CONTROLS_BY_ID = {}
for k, def in pairs(CONTROLS) do
    def.key = k
    CONTROLS_BY_ID[def.id] = def
end

local ERR_BITS = { [0] = "数据非法", [1] = "无请求数据项", [2] = "密码错/未授权", [3] = "远程控制失败" }

--- 把引擎返回的 "电表异常: 0xNN" 翻译成 B.3.2 的错误含义
local function explain_err(err)
    local code = tonumber(tostring(err):match("电表异常: 0x(%x+)"), 16)
    if not code then
        return tostring(err)
    end
    local parts = {}
    for bit = 0, 3 do
        if code & (1 << bit) ~= 0 then
            parts[#parts + 1] = ERR_BITS[bit]
        end
    end
    return string.format("%s（%s）", tostring(err), table.concat(parts, "/"))
end

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

local poll -- 前向声明，on_write 下发后回读用

local function send_control(def)
    local c = ensure_conn()
    if not c then
        return nil, "未连接"
    end
    local cc = CONFIG.control
    local data = {}
    for _, b in ipairs(def.di) do data[#data + 1] = b end
    data[#data + 1] = cc.pa
    for _, b in ipairs(cc.pw) do data[#data + 1] = b end
    for _, b in ipairs(cc.operator) do data[#data + 1] = b end
    -- DATA：NN=00（立即执行），uu=单位
    if cc.data_order == "nn_first" then
        data[#data + 1] = 0x00
        data[#data + 1] = cc.unit
    else
        data[#data + 1] = cc.unit
        data[#data + 1] = 0x00
    end
    return c:request(0x1C, data, CONFIG.link.address)
end

local function on_write(points)
    local acted = false
    for _, p in ipairs(points) do
        local def = CONTROLS[p.key or ""] or CONTROLS_BY_ID[p.id or -1]
        if not def then
            log.warn("[" .. CONFIG.dev_id .. "] 不支持下发的点位: " .. tostring(p.key or p.id or p.name))
        elseif p.value ~= 1 then
            log.warn(string.format("[%s] %s 下发值 %s 被忽略（仅 1 触发）", CONFIG.dev_id, def.name, tostring(p.value)))
        else
            log.warn(string.format("[%s] 执行%s", CONFIG.dev_id, def.name))
            local ok, err = send_control(def)
            if ok then
                acted = true
                log.warn(string.format("[%s] %s 成功（设备已应答 9CH）", CONFIG.dev_id, def.name))
            else
                -- 超时时命令可能已执行，靠下面的回读确认闸位
                log.error(string.format("[%s] %s 失败: %s", CONFIG.dev_id, def.name, explain_err(err)))
                acted = true
            end
        end
    end
    if acted then
        wait(500) -- 给设备动作时间再回读闸位
        poll()
    end
end

local dev, derr = dc.register(CONFIG.dev_id, on_write)
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

poll = function()
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
