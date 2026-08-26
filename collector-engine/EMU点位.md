EMU功能点位，点在DataCenter中流转

|点位|key|name|
|----|----|---|
|1|operation_mode|EMU运行模式|
|2|permission|EMU充放电许可|
|3|health_status|EMU告警故障状态|
|4|charge_soc_limit|充电SOC限制|
|5|discharge_soc_limit|放电SOC限制|
|6|planned_curve|计划曲线使能|
|7|emu_power|EMU上电方式|
|10|sys_tms_mode|系统热管理模式|
|500-2000|warn_fault|告警故障|

点位ID/KEY常量定义：`collector-engine/src/emu/mod.rs`

---

## 1. operation_mode（EMU运行模式）

- 类型：枚举 `u8`（`collector-core/src/runtime/emu.rs`，`OperationMode`）
- 枚举值：
  - `0` Standby 静置
  - `1` Charging 充电中
  - `2` Discharging 放电中
- 逻辑：`collector-engine/src/emu/emu_runtime.rs` 每秒读取 bcu 寄存器34（充放电状态）换算得出，写入 emu 设备。
- 用途：反映电池系统当前处于静置/充电/放电哪种运行状态。

## 2. permission（EMU充放电许可）

- 类型：枚举 `u8`（`collector-core/src/runtime/emu.rs`，`EmuPermission`）
- 枚举值：
  - `0` Normal 正常
  - `1` ChargeDisabled 禁充
  - `2` DischargeDisabled 禁放
  - `3` TotalStop 禁充禁放（仅作为初始值，代码中未见主动赋值路径）
- 逻辑：`collector-engine/src/emu/emu_runtime.rs` 根据当前SOC与 `charge_soc_limit`/`discharge_soc_limit` 比较得出：SOC达到充电限值→禁充（并强制下发PCS功率0）；SOC达到放电限值→禁放（同样强制下发功率0）；否则正常。
- 用途：控制/展示当前系统是否允许充电或放电，联动PCS功率下发实现SOC保护。

## 3. health_status（EMU告警故障状态）

- 类型：枚举 `u8`（`collector-core/src/runtime/emu.rs`，`HealthStatus`）
- 枚举值：
  - `0` Normal 正常
  - `1` Warning 告警
  - `2` Alarm 故障
- 逻辑：`collector-engine/src/emu/fault.rs` 中 `FaultDiagnosis` 策略每3秒扫描 pcs/bcu/tms 各故障寄存器展开出的告警位（见第9节 warn_fault），按 `WarnLevel` 聚合：命中 High → Warning；命中 Critical → Alarm（优先级最高）；均未命中 → Normal。
- 用途：系统整体健康度的三级汇总指示，由明细告警位（warn_fault）聚合而来。

## 4. charge_soc_limit（充电SOC限制）

- 类型：`f64`，非枚举
- 默认值：`95.0`（`collector-core/src/runtime/emu.rs`，`SocProtect::new()`）
- 逻辑：可通过下行点位修改，修改后持久化到 `./config/emu_runtime_config.json`。
- 用途：SOC达到此限值时触发 `permission=ChargeDisabled`（禁充）。

## 5. discharge_soc_limit（放电SOC限制）

- 类型：`f64`，非枚举
- 默认值：`5.0`（`SocProtect::new()`）
- 逻辑：与 charge_soc_limit 对称，可下行修改并持久化。
- 用途：SOC低于此限值时触发 `permission=DischargeDisabled`（禁放）。

## 6. planned_curve（计划曲线使能）

- 类型：`bool`（编码为 `Val::U8(0/1)`）
- 实现：`collector-engine/src/emu/planned_curve.rs`（策略，1分钟一次）+ `collector-core/src/runtime/planned_curve.rs`（状态持久化于 SQLite 表 `t_emu_function`，`function_code='PLAN_CURVE'`）
- 逻辑：使能开启后，从 SQLite 表 `t_plan_curve_master`/`t_plan_curve_detail` 查找当前生效的计划曲线（按有效期/星期），按15分钟粒度（`time_index` 0-95，对应00:00-23:45）读取该时段目标有功功率 `power_value` 及可选 `soc_limit`；若SOC已达限制则下发功率0，否则下发计划功率给pcs。
- 用途：控制是否启用"计划曲线"（分时功率计划）功能。

## 7. emu_power（EMU上电方式）

- 类型：命令型点位（整数，非常态上行状态量），实现于 `collector-engine/src/emu/cmd.rs`（`EmuPower` Command）
- 取值含义：
  - `1` 并网启动（grid_on_start）：启动bcu → 等待PCS通信建立 → 设置PCS远程 → 清除故障 → 开机
  - `2` 黑启动/离网启动（grid_off_start）：启动bcu → 等待PCS通信 → 设置远程/清除故障/VF离网模式/离网输出给定电压400V/开机命令
  - 其他值：无动作
- 用途：EMU系统整体上电启动方式选择（并网启动 vs 离网黑启动），一次性触发命令。

## 8. sys_tms_mode（系统热管理模式）

- 类型：枚举 `u8`（`collector-engine/src/emu/tms.rs`，`SysTmsMode`）
- 枚举值：
  - `0` Manual 手动
  - `1` Auto 自动
  - `2` Circulation 自循环
  - `3` Level1Cooling 一级制冷
  - `4` Level2Cooling 二级制冷
  - `5` Heating 制热
  - `6` Standby 待机
- 自动模式判定逻辑（基于bcu电池最高/最低/平均温度 `t_max`/`t_min`/`t_vag`）：
  - BCU通讯断开或温度数据异常 → 回退一级制冷/22°C
  - `t_max∈[25,28)` 且 `t_vag∈[22,28)` → 自循环（仅水泵运行）
  - `t_min<=10` 且 `t_vag<=15` → 制热（出水温度15°C）
  - `t_max∈[28,34)` 且 `t_vag>=26` → 一级制冷（出水温度24°C）
  - `t_max>=34` 且 `t_vag>=28` → 二级制冷（出水温度22°C）
  - 否则 → 待机（关闭TMS使能）
- 下行控制：可下发本点位手动切换到以上任一模式。
- 底层对tms设备的实际动作寄存器：`id 2000` 热管理使能(bool)，`id 2001` 模式（Cooling=1/Heating=2/Circulation=3），`id 2002` 制冷出水温度设定，`id 2004` 制热出水温度设定。
- 用途：液冷机组（水冷系统）运行模式管理，依据电池温度自动切换制冷/制热/自循环/待机。

## 9. warn_fault（告警故障，点位范围500-2000）

代码中没有名为 `warn_fault` 的常量，该key是对500-2000这段ID区间的统称；实际每个具体故障位都有各自独立的key（取自设备Excel配置中告警位的英文名）。

- 动态生成逻辑：`collector-engine/src/emu/fault.rs`（`FaultDiagnosis` 策略，3秒一次）
  - 读取原始故障字寄存器：
    - pcs：id `[156,157,158,159,160,164,165]`（5个控制软件故障字 + 2个通讯软件故障字，各16bit）
    - bcu：id `[100..121]`（其他报警信息、Rack接触器状态、主控/从控故障、功能安全告警、均衡错误、BMU通信/概要故障、Rack严重/中度/轻微报警等，共22个寄存器×16bit）
    - tms：id `[20,21,22,23]`（故障代码1~4，各16bit）
  - 对每个寄存器逐位展开，过滤掉 `level==None` 的普通状态位，为每个命中的告警位生成一个新DataPoint（`key`=位的英文名，`name`=位的中文名，`value`=Val::U8(0/1)），统一分配 `id = 500 + 序号`。
  - 同时按位的 `WarnLevel` 联动更新 `health_status`。
- 告警级别 `WarnLevel`（`collector-core/src/core/point.rs`）：
  - `0` None 非告警位（普通状态位）
  - `1` Normal 一般/轻微
  - `2` High 较严重（触发 health_status=Warning）
  - `3` Critical 严重故障（触发 health_status=Alarm）
- 告警位明细来源（非硬编码，来自Excel配置）：
  - PCS：`config/PCS_125_英博.xlsx`（遥测sheet）。主要故障：A/B/C/N相硬件过流、辅助源掉电、绝缘故障、驱动故障、散热器/环境过温、输出过载过流、交流过压欠压/缺相/相序错误、直流反接、母线软件过压欠压、交流过频欠频、直流充放电过流、SPI通信故障、继电器合分闸故障、铁电参数保存错误、直流软起失败、电网孤岛故障、启动超时/条件不满足、支路/直路/交流不平衡、保险熔断、光纤通讯故障、电池软件过欠压、漏电流超限、零漂、ARM各类内部/EMS/BMS通讯故障、SD卡/RTC故障、急停信号、STS开关晶闸管过温等（绝大多数为级别2）。
  - BCU：`config/永泰_2_BCU.xlsx`（信号sheet，CAN协议）。主要故障：从控概要故障、NTC故障、接触器粘连、内网通信故障、EEPROM故障、电流/绝缘检测故障、采样线/采样芯片/电压/温度采样故障、被动/主动均衡故障、极柱温度故障、总压差过大、单体电压/温度过大、SOC低、Rack级严重（level3）/中度（level2）/轻微（level1）报警（总压/单体过欠压、放充电电流/温度过大、绝缘阻值过低、压差温差过大等）。
  - TMS：`config/水冷机组_埃森特.xlsx`（遥测sheet）。主要故障：各类温度/压力传感器故障、系统高压告警、水泵故障、排气过热度低/压缩比异常、压缩机过流过温保护、水温/水压过高过低保护、滤网堵塞、水箱液位过低、风机1-4故障/离线、内存(E2)错误、驱动离线等（绝大多数为级别2）。
- 消费端：`collector-api/src/handlers/ws.rs`（`HomeWarnDatas::new`）调用 `center.read_range("emu", 500, 2000)`，过滤 `value==1` 的当前触发中告警，推送给前端首页告警列表。

---

## 相关文件一览

| 文件 | 作用 |
|---|---|
| `collector-engine/src/emu/mod.rs` | 各点位ID/KEY常量定义 |
| `collector-engine/src/emu/emu_runtime.rs` | operation_mode/permission/health_status/charge_soc_limit/discharge_soc_limit 采集与写点逻辑 |
| `collector-engine/src/emu/planned_curve.rs` | planned_curve 点位与计划曲线执行逻辑 |
| `collector-engine/src/emu/cmd.rs` | emu_power 命令处理（并网/黑启动） |
| `collector-engine/src/emu/tms.rs` | sys_tms_mode 枚举、自动判定逻辑、下行控制 |
| `collector-engine/src/emu/fault.rs` | warn_fault(500-2000) 动态展开、health_status 联动 |
| `collector-engine/src/emu/taos.rs` | pcs/bcu/tms 原始寄存器列定义（用于TDengine落库） |
| `collector-core/src/runtime/emu.rs` | OperationMode/EmuPermission/HealthStatus 枚举、RuntimeEmu（运行时状态+SOC保护配置持久化） |
| `collector-core/src/runtime/planned_curve.rs` | RuntimePlannedCurve（计划曲线使能持久化，SQLite表 t_emu_function） |
| `collector-core/src/core/point.rs` | DataPoint/Bits/Bit/WarnLevel 通用定义、告警位解析 |
| `collector-api/src/handlers/ws.rs` | 首页告警列表API，读取 read_range("emu",500,2000) |
| `config/*.xlsx` | 实际故障码/告警位明细数据源 |
