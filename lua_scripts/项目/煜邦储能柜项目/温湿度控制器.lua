-- 温湿度控制器（Modbus）：按设备寄存器表直连采集与控制，注册为数据中心设备。
--
-- 寄存器地址均为协议 0 基地址，由文档编号换算：
--   FC01/FC05 状态位：地址 = 编号 - 10001（如 控湿回路状态 10035 -> 34）
--   FC03/FC06 参数  ：地址 = 编号 - 30001 / 40001（如 环境温度 30002 -> 1，控湿开启值 40011 -> 10）
--
-- 启用方式：在 _manifest.lua 的返回列表里加入 "temp_humi_controller.lua"，并按实际设备修改 CONFIG。
-- 注意：CONFIG.dev_id 与串口不能和 modbus_dev 已配置的设备重复，否则内置设备优先，本脚本注册/连接失败。
--
-- 下发（dc.dispatch(dev_id, key, value)）可用的 key：
--   参数：tempOn 控温开启值、tempStop 控温停止值、humiOn 控湿开启值、humiStop 控湿停止值、
--         tempAlarmHigh 温度报警上限、tempAlarmLow 温度报警下限、dewOn 露点启动值、dewHyst 露点回差值（单位 ℃ / %RH，直接传工程值）
--   控制：heatMode 控温方式（1=升温，0=降温，与读回一致；脚本内部按文档转换为 0000/FF00）、
--         humiManual 控湿手动开关（1=手动开，0=自动）、tempManual 控温手动开关（1=手动开，0=自动）
MOD = {
    name = "温湿度控制器",
    description = "Modbus 温湿度控制器：采集温湿度/回路状态/参数，支持参数设置与手动控制",
}

local CONFIG = {
    dev_id = "temp_humi_ctrl",
    poll_ms = 2000,
    -- RTU 示例（串口参数请按设备实际设置）；改 TCP 则用 { type = "tcp", host = "192.168.1.10", port = 502, slave = 1 }
    -- link = { type = "rtu", serial = "/dev/ttyS1", baud = 9600, parity = "N", slave = 1, timeout = 800 },
    link = { type = "tcp", host = "192.168.0.220", port = 503, slave = 3 },
    -- 型号差异开关
    has_mist = false,    -- GCK 型号：雾化模块状态/雾化工作状态/内部温度2
    dual_output = false, -- 带两路输出：故障回路状态
    has_dewpoint = false, -- 支持露点：露点温度/露点参数/露点回路状态
}

-----------------------------------------------------------------------
-- 点表
-----------------------------------------------------------------------

-- FC03 模拟量，变比 /10。内部温度值（地址 0）文档标注无需读，故从地址 1 开始读
local ANALOG_ADDR, ANALOG_COUNT = 1, 4
local ANALOG = {
    { addr = 1, id = 1, key = "envTemp", name = "环境温度", unit = "℃", signed = true },
    { addr = 2, id = 2, key = "envHumi", name = "环境湿度", unit = "%RH" },
    { addr = 3, id = 3, key = "intTemp2", name = "内部温度2", unit = "℃", signed = true, opt = "has_mist" },
    { addr = 4, id = 4, key = "dewPoint", name = "露点温度", unit = "℃", signed = true, opt = "has_dewpoint" },
}

-- FC03 读 / FC06 写 运行参数，变比 /10，min/max 为工程值范围，integer 表示小数位必须为 0
local PARAM_ADDR, PARAM_COUNT = 8, 8
local PARAMS = {
    { addr = 8, id = 11, key = "tempOn", name = "控温开启值", unit = "℃", min = 1, max = 85, integer = true },
    { addr = 9, id = 12, key = "tempStop", name = "控温停止值", unit = "℃", min = 1, max = 85, integer = true },
    { addr = 10, id = 13, key = "humiOn", name = "控湿开启值", unit = "%RH", min = 25, max = 98, integer = true },
    { addr = 11, id = 14, key = "humiStop", name = "控湿停止值", unit = "%RH", min = 25, max = 98, integer = true },
    { addr = 12, id = 15, key = "tempAlarmHigh", name = "温度报警上限", unit = "℃", min = 60, max = 95 },
    { addr = 13, id = 16, key = "tempAlarmLow", name = "温度报警下限", unit = "℃", min = 30, max = 60 },
    { addr = 14, id = 17, key = "dewOn", name = "露点启动值", unit = "℃", min = 5, max = 80, opt = "has_dewpoint" },
    { addr = 15, id = 18, key = "dewHyst", name = "露点回差值", unit = "℃", min = 1, max = 10, opt = "has_dewpoint" },
}

-- FC01 读 / FC05 写 运行状态位，连续读 COIL_COUNT 个。
-- ctrl：nil=只读；"direct"=1->FF00 开启、0->0000 关闭；"invert"=1->0000、0->FF00（控温方式：文档中 0000=升温、FF00=降温）
local COIL_ADDR, COIL_COUNT = 31, 16
local COILS = {
    { addr = 31, id = 21, key = "mistFault", name = "雾化模块故障", opt = "has_mist" },
    { addr = 32, id = 22, key = "mistWorking", name = "雾化工作状态", opt = "has_mist" },
    { addr = 33, id = 23, key = "faultCircuit", name = "故障回路状态", opt = "dual_output" },
    { addr = 34, id = 24, key = "humiRunning", name = "控湿回路状态" },
    { addr = 35, id = 25, key = "tempRunning", name = "控温回路状态" },
    { addr = 36, id = 26, key = "heatMode", name = "控温方式", ctrl = "invert" }, -- 1=升温 0=降温
    { addr = 37, id = 27, key = "fanFault", name = "风机模块回路故障" },
    { addr = 38, id = 28, key = "dehumiFault", name = "除湿模块回路故障" },
    { addr = 39, id = 29, key = "humiSensorFault", name = "湿度传感器故障" },
    { addr = 40, id = 30, key = "extTempSensorFault", name = "外部温度传感器故障" },
    { addr = 41, id = 31, key = "intTempSensorFault", name = "内部温度传感器故障" },
    { addr = 42, id = 32, key = "defrosting", name = "化霜状态" },
    { addr = 43, id = 33, key = "humiManual", name = "控湿手动开关", ctrl = "direct" }, -- 1=手动开 0=自动
    { addr = 44, id = 34, key = "tempManual", name = "控温手动开关", ctrl = "direct" },
    { addr = 45, id = 35, key = "highTempAlarm", name = "高温告警" },
    { addr = 46, id = 36, key = "dewRunning", name = "露点回路工作状态", opt = "has_dewpoint" },
}

local function enabled(def)
    return def.opt == nil or CONFIG[def.opt]
end

-- key / id -> 可下发点位定义
local WRITABLE = {}
for _, def in ipairs(PARAMS) do
    if enabled(def) then
        def.kind = "param"
        WRITABLE[def.key], WRITABLE[def.id] = def, def
    end
end
for _, def in ipairs(COILS) do
    if enabled(def) and def.ctrl then
        def.kind = "coil"
        WRITABLE[def.key], WRITABLE[def.id] = def, def
    end
end

-----------------------------------------------------------------------
-- 连接与状态
-----------------------------------------------------------------------

local TAG = "[" .. CONFIG.dev_id .. "] "
local conn
local state = {} -- key -> 最近一次读到的工程值，用于下发前的联动校验

local function ensure_conn()
    if conn then
        return conn
    end
    local c, err = modbus.connect(CONFIG.link)
    if not c then
        log.warn(TAG .. "连接失败: " .. tostring(err))
        return nil
    end
    conn = c
    return conn
end

local on_write -- 前向声明，实现见"下发"一节
local dev, derr = dc.register(CONFIG.dev_id, function(points)
    return on_write(points)
end)
if not dev then
    log.error(TAG .. "注册设备失败: " .. tostring(derr))
    return
end

-----------------------------------------------------------------------
-- 采集
-----------------------------------------------------------------------

local function poll()
    local c = ensure_conn()
    if not c then
        dev:set_comm(false)
        return false
    end

    local points, all_ok = {}, true
    local function add(def, value, ty)
        state[def.key] = value
        points[#points + 1] = { id = def.id, key = def.key, name = def.name, unit = def.unit, value = value, type = ty }
    end

    local regs, err = c:read_holding(ANALOG_ADDR, ANALOG_COUNT)
    if regs then
        for _, def in ipairs(ANALOG) do
            if enabled(def) then
                local i = def.addr - ANALOG_ADDR + 1
                local raw = def.signed and modbus.decode(regs, "I16", nil, i) or regs[i]
                add(def, raw / 10, "f64")
            end
        end
    else
        all_ok = false
        log.warn(TAG .. "读取模拟量失败: " .. tostring(err))
    end

    regs, err = c:read_holding(PARAM_ADDR, PARAM_COUNT)
    if regs then
        for _, def in ipairs(PARAMS) do
            if enabled(def) then
                add(def, regs[def.addr - PARAM_ADDR + 1] / 10, "f64")
            end
        end
    else
        all_ok = false
        log.warn(TAG .. "读取运行参数失败: " .. tostring(err))
    end

    local bits
    bits, err = c:read_coils(COIL_ADDR, COIL_COUNT)
    if bits then
        for _, def in ipairs(COILS) do
            if enabled(def) then
                add(def, bits[def.addr - COIL_ADDR + 1] and 1 or 0, "u8")
            end
        end
    else
        all_ok = false
        log.warn(TAG .. "读取运行状态失败: " .. tostring(err))
    end

    if #points > 0 then
        dev:ingest(points)
    end
    dev:set_comm(all_ok)
    return all_ok
end

-----------------------------------------------------------------------
-- 下发
-----------------------------------------------------------------------

-- 校验参数取值；联动规则依据寄存器表注：
--   控温停止值：升温时 开启值+1 ~ +20，降温时 开启值-1 ~ -20；控湿停止值：开启值-3 ~ -20；
--   控温开启/停止值、控湿开启/停止值小数位必须为 0。联动校验依赖最近一次读到的值，缺失时跳过，由设备自行拒绝
local function check_param(def, v)
    if v ~= v then
        return "不是有效数字"
    end
    if v < def.min or v > def.max then
        return string.format("取值范围 %g~%g", def.min, def.max)
    end
    if def.integer and v ~= math.floor(v) then
        return "小数位必须为 0"
    end
    local lo, hi
    if def.key == "tempStop" and state.tempOn and state.heatMode then
        if state.heatMode == 1 then
            lo, hi = state.tempOn + 1, state.tempOn + 20
        else
            lo, hi = state.tempOn - 20, state.tempOn - 1
        end
    elseif def.key == "humiStop" and state.humiOn then
        lo, hi = state.humiOn - 20, state.humiOn - 3
    end
    if lo and (v < lo or v > hi) then
        return string.format("需在 %g~%g 之间（相对当前开启值）", lo, hi)
    end
    return nil
end

local function write_one(c, def, v)
    if def.kind == "param" then
        local why = check_param(def, v)
        if why then
            return nil, why
        end
        return c:write_register(def.addr, math.floor(v * 10 + 0.5))
    end
    local on = v ~= 0
    if def.ctrl == "invert" then
        on = not on
    end
    return c:write_coil(def.addr, on)
end

on_write = function(points)
    local c = ensure_conn()
    if not c then
        log.warn(TAG .. "未连接，丢弃下发")
        return
    end
    local wrote = false
    for _, p in ipairs(points) do
        local def = WRITABLE[p.key or p.id]
        if not def then
            log.warn(TAG .. "不支持下发的点位: " .. tostring(p.key or p.id or p.name))
        else
            local ok, err = write_one(c, def, p.value)
            if ok then
                wrote = true
                log.info(TAG .. "下发 " .. def.key .. " = " .. tostring(p.value))
            else
                log.warn(TAG .. "下发 " .. def.key .. " 失败: " .. tostring(err))
            end
        end
    end
    if wrote then
        poll() -- 立即回读，让下发结果尽快反映到数据中心
    end
end

task.spawn(function()
    while true do
        poll()
        wait(CONFIG.poll_ms)
    end
end)
