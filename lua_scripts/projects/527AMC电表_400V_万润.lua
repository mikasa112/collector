-- AMC 多功能表（Modbus RTU）：分快慢两条报文读取，快段 0x0023~0x0035（电压/电流/功率）每次轮询都读，
--   慢段 0x0036~0x0042（功率因数/视在功率/频率/吸收释放有功电能）每 SLOW_EVERY 次轮询读一次，各表相位错开；另读 PT/CT 变比，注册为数据中心设备。
-- 故障逻辑：各读取段互相独立，某段读失败/超时只跳过该段的点（沿用数据中心里的上一次值），继续读下一段；
--   只有所有点都没读到（本轮 0 个有效点）才算一次通讯失败，连续 offline_after 次才置通讯故障。
-- 两块 10kV 关口表分别挂在不同串口：/dev/ttyS5（从站 4）、/dev/ttyS4（从站 5），各自一个连接，串行轮询。
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
    poll_ms = 2000,
    slave_gap_ms = 100, -- 相邻从站请求之间留出的总线空闲时间
    read_retries = 2, -- 单次读取失败（超时/CRC 错）后的额外重试次数，现场 RS485 链路有约 25% 的帧损坏
    retry_gap_ms = 50, -- 重试前等待，让总线上残留的半帧收完
    offline_after = 3, -- 连续读取失败多少次才置通讯故障，滤掉总线偶发超时
    -- 每块表独立串口；dev_id 全局唯一
    meters = {
        { dev_id = "amc_meter_1", slave = 4,
          link = { type = "rtu", serial = "/dev/ttyS5", baud = 9600, parity = "N", timeout = 2000 } },
        { dev_id = "amc_meter_2", slave = 5,
          link = { type = "rtu", serial = "/dev/ttyS4", baud = 9600, parity = "N", timeout = 2000 } },
    },
}

-- 读取段：每段一次请求，失败只影响本段内的点；段内寄存器必须连续。
-- 按实时性拆成两段：every 为每多少次轮询读一次（缺省 1=每次）；未读的轮次沿用数据中心里的上一次值
--   快段：小数位寄存器 + 电压/电流/有功/无功功率（0x0023~0x0035），每次轮询都读
--   慢段：功率因数/视在功率/频率/有功电能（0x0036~0x0042），每 SLOW_EVERY 次轮询读一次
local SLOW_EVERY = 5
local SEGMENTS = {
    { name = "实时量", addr = 0x0023, count = 0x0036 - 0x0023 },
    { name = "慢速量", addr = 0x0036, count = 0x0043 - 0x0036, every = SLOW_EVERY },
}
-- 电能点：addr 为二次侧 Uint32 起始地址（高字在前）
local ENERGY_POINTS = {
    { addr = 0x003F, id = 27, key = "EPimp", name = "吸收有功电能" },
    { addr = 0x0041, id = 28, key = "EPexp", name = "释放有功电能" },
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
local polls = {} -- dev_id -> 已轮询次数，用于慢段分频
local scales = {} -- dev_id -> 缓存的功率换算系数（慢段的视在功率依赖快段的小数位寄存器）
local ratios = {} -- dev_id -> { pt =, ct =, age = 距上次读取的成功采集次数 }

local conns = {} -- dev_id -> 连接；modbus.connect 失败时为 nil，轮询里会重试

local devs = {}
for idx, m in ipairs(CONFIG.meters) do
    -- 慢段相位：各表均匀错开，避免同一轮同时读慢段（首轮各表都读一次）
    m.slow_offset = math.floor((idx - 1) * SLOW_EVERY / #CONFIG.meters)
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

-- 读保持寄存器，失败自动重试 CONFIG.read_retries 次；每次读完都 wait(1) 让出以重置 200ms 脚本预算
-- （引擎对脚本单次执行只给 200ms 预算，而串口读取的等待时间也计入其中，不让出会被判超时、协程被杀、数据停更）
local function read_regs(m, addr, count)
    local regs, err
    for attempt = 0, CONFIG.read_retries do
        if attempt > 0 then wait(CONFIG.retry_gap_ms) end
        regs, err = conns[m.dev_id]:read_holding(addr, count, m.slave)
        wait(1)
        if regs then return regs, nil, attempt end
    end
    return nil, err, CONFIG.read_retries
end

-- 读取并缓存 PT/CT；读取失败时保留旧缓存，从未读到过则返回 nil（电能点不上报，避免上报错误倍率的值）
local function get_ratio(m)
    local r = ratios[m.dev_id]
    if r and r.age < RATIO_REFRESH_POLLS then
        r.age = r.age + 1
        return r
    end
    wait(CONFIG.slave_gap_ms)
    local regs, err = read_regs(m, RATIO_ADDR, 2)
    if regs and regs[1] and regs[2] and regs[1] >= 1 and regs[2] >= 1 then
        r = { pt = regs[1], ct = regs[2], age = 0 }
        ratios[m.dev_id] = r
    else
        log.warn("[" .. m.dev_id .. "] 读取PT/CT失败: " .. tostring(err))
    end
    return r
end

-- 段描述：段名、寄存器范围、受影响的点位（id:key），用于日志定位具体是哪些点读失败
local function seg_desc(seg)
    local last = seg.addr + seg.count - 1
    local pts = {}
    for _, def in ipairs(POINTS) do
        if def.addr >= seg.addr and def.addr <= last then
            pts[#pts + 1] = def.id .. ":" .. def.key
        end
    end
    for _, e in ipairs(ENERGY_POINTS) do
        if e.addr >= seg.addr and e.addr + 1 <= last then
            pts[#pts + 1] = e.id .. ":" .. e.key
        end
    end
    return string.format("%s[0x%04X~0x%04X] 点位=%s", seg.name, seg.addr, last, table.concat(pts, ","))
end

-- 依次读取各段，失败的段跳过；返回 地址->寄存器值 表和失败段名列表
local function read_segments(m)
    local words, failed, last_err = {}, {}, nil
    local n, read_any = polls[m.dev_id], false
    for _, seg in ipairs(SEGMENTS) do
      if seg.every == nil or n == 1 or (n - 1 - (m.slow_offset or 0)) % seg.every == 0 then
        if read_any then wait(CONFIG.slave_gap_ms) end
        read_any = true
        local regs, err, tries = read_regs(m, seg.addr, seg.count)
        if regs and tries > 0 then
            log.info("[" .. m.dev_id .. "] " .. seg.name .. " 重试 " .. tries .. " 次后读取成功")
        end
        if regs then
            for k = 1, seg.count do
                words[seg.addr + k - 1] = regs[k]
            end
        else
            failed[#failed + 1] = seg_desc(seg)
            last_err = err
        end
      end
    end
    return words, failed, last_err
end

local function poll_meter(m, dev)
    local tag = "[" .. m.dev_id .. "] "
    polls[m.dev_id] = (polls[m.dev_id] or 0) + 1
    local words, failed, err = read_segments(m)

    -- 换算系数：本轮读到则更新缓存，未读到（慢段轮次/快段失败）用缓存
    local sc = scales[m.dev_id] or {}
    scales[m.dev_id] = sc
    local r23, r24 = words[0x0023], words[0x0024]
    if r23 then
        local dpt, dct = math.floor(r23 / 256), r23 % 256
        sc.k_u, sc.k_i, sc.dec_u, sc.dec_i = 10 ^ (dpt - 4), 10 ^ (dct - 4), decimals_of(dpt), decimals_of(dct)
    end
    if r24 then
        local dpq = math.floor(r24 / 256)
        sc.k_p, sc.dec_p = 10 ^ (dpq - 4), decimals_of(dpq)
        sc.sign_bits = r24 % 256
    end
    local k_u, k_i, k_p, dec_u, dec_i, dec_p, sign_bits =
        sc.k_u, sc.k_i, sc.k_p, sc.dec_u, sc.dec_i, sc.dec_p, sc.sign_bits

    local out = {}
    for _, def in ipairs(POINTS) do
        local raw = words[def.addr]
        local v
        if raw ~= nil then
            if def.kind == "U" then
                v = k_u and round(raw * k_u, dec_u)
            elseif def.kind == "I" then
                v = k_i and round(raw * k_i, dec_i)
            elseif def.kind == "P" then
                if k_p then
                    v = round(raw * k_p / 1000, dec_p + 3) -- W -> kW
                    if def.sign and math.floor(sign_bits / 2 ^ def.sign) % 2 == 1 then
                        v = -v
                    end
                end
            else
                v = round(raw * def.scale, def.decimals)
            end
        end
        if v ~= nil then
            out[#out + 1] = {
                id = def.id, key = def.key, name = def.name, unit = def.unit,
                type = "f64", value = v,
            }
        end
    end

    -- 有功电能（一次侧 kWh）：电能寄存器和 PT/CT 都读到才上报
    if words[ENERGY_POINTS[1].addr] ~= nil then
        local ratio = get_ratio(m)
        if ratio then
            for _, e in ipairs(ENERGY_POINTS) do
                local hi, lo = words[e.addr], words[e.addr + 1]
                if hi ~= nil and lo ~= nil then
                    out[#out + 1] = {
                        id = e.id, key = e.key, name = e.name, unit = "kWh",
                        type = "f64", value = round((hi * 65536 + lo) / 1000 * ratio.pt * ratio.ct, 3),
                    }
                end
            end
        end
    end

    if #failed > 0 then
        log.warn(tag .. "slave=" .. m.slave .. " " .. m.link.serial .. " 部分读取失败，已跳过: "
            .. table.concat(failed, "；") .. "，最近错误: " .. tostring(err))
    end

    if #out == 0 then
        -- 本轮没有任何有效点：算一次通讯失败，连续多次才置通讯故障
        local n = (fail_count[m.dev_id] or 0) + 1
        fail_count[m.dev_id] = n
        log.warn(tag .. "全部点读取失败(连续" .. n .. "次)")
        if n >= CONFIG.offline_after then
            dev:set_comm(false)
        end
        return
    end
    fail_count[m.dev_id] = 0
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
        for _, d in ipairs(devs) do
            if d.every == 1 or tick % d.every == d.phase then
                local id = d.m.dev_id
                if not conns[id] then
                    local c, err = modbus.connect(d.m.link)
                    wait(1) -- 连接耗时同样计入预算
                    if c then
                        conns[id] = c
                    else
                        log.warn("[" .. id .. "] 连接失败: " .. tostring(err))
                        d.dev:set_comm(false)
                    end
                end
                if conns[id] then
                    poll_meter(d.m, d.dev)
                    wait(CONFIG.slave_gap_ms)
                end
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
                conns = {}
                collectgarbage()
            end
            busy = false
        end)
    end)
end
