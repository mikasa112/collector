EMU功能点位，点在DataCenter中流转

|点位|key|name|读写|
|----|----|---|----|
|1|operation_mode|EMU充放电状态|只读|
|2|permission|EMU充放电许可|只读|
|3|health_status|EMU告警故障状态|只读|
|4|charge_soc_limit|充电SOC限制|可读可写|
|5|discharge_soc_limit|放电SOC限制|可读可写|
|6|planned_curve|计划曲线使能|可读可写|
|7|emu_power|EMU上电方式|可读可写|
|8|run_mode|EMU运行模式|可读可写|
|9|control_source|EMU控制源|可读可写|
|10|sys_tms_mode|系统热管理模式|可读可写|
|11|anti_backflow_enable|防逆流使能|可读可写|
|12|anti_backflow_threshold|防逆流功率阈值（kW）|可读可写|
|13|anti_backflow_hysteresis|防逆流功率回差（kW）|可读可写|
|14|demand_guard_enable|需量保护使能|可读可写|
|15|demand_limit|需量限制（kW）|可读可写|
|16|demand_hysteresis|需量保护功率回差（kW）|可读可写|
|17|grid_mode|并离网状态|只读|
|500-2000|warn_fault|告警故障|只读|

点位ID/KEY常量定义：`collector-engine/src/emu/mod.rs`

EMU 启动（`collector-engine/src/emu/core.rs`，`Emu::new`）时注册的策略：EMU运行时策略(1s)、故障诊断(3s)、热管理(60s)、计划曲线(60s)、TDengine落库、功率保护(1s)；命令：EMU上电方式。

EMU 读写外部设备点位不再硬编码设备/点位ID，而是通过 `field_registry()` 的**逻辑字段**（`mod.rs` 中 `FIELDS`，可在字段绑定页/覆盖表中改绑，`reload` 后即时生效）：

| 字段key | 含义 | 方向 | 默认绑定 |
|---|---|---|---|
| bcu_comm_status | BCU充放电状态（见第1节） | 读 | bcu/34 |
| soc | SOC | 读 | bcu/32 |
| bcu_current | BCU电流（负充正放） | 读 | bcu/46 |
| pcs_grid_connected | PCS并网状态（1并网） | 读 | pcs/1007 |
| pcs_off_grid | PCS VF离网状态（1离网） | 读 | pcs/1008 |
| pcs_active_power | PCS有功功率设定（正充负放） | 读写 | pcs/2003 |
| grid_active_power | 关口表有功功率（取电正、反送负） | 读 | grid_meter/9999（**占位**，上线前须改绑真实点位） |
| pcs_actual_power | PCS总输出有功功率（点位口径负充正放） | 读 | pcs/11 |

---

## 1. operation_mode（EMU充放电状态）

- 类型：枚举 `u8`（`collector-core/src/runtime/emu.rs`，`OperationMode`）
- 枚举值：
  - `0` Standby 静置
  - `1` Charging 充电中
  - `2` Discharging 放电中
- 逻辑：`collector-engine/src/emu/emu_runtime.rs` 每秒读取逻辑字段 `bcu_comm_status`（默认 bcu 寄存器34）换算得出，写入 emu 设备。换算关系：寄存器值 `0`→静置，`1`→放电中，`2`→充电中，其他→静置（注意与枚举值的1/2顺序相反）。
- 用途：反映电池系统当前处于静置/充电/放电哪种运行状态。

## 2. permission（EMU充放电许可）

- 类型：枚举 `u8`（`collector-core/src/runtime/emu.rs`，`EmuPermission`）
- 枚举值：
  - `0` Normal 正常
  - `1` ChargeDisabled 禁充
  - `2` DischargeDisabled 禁放
  - `3` TotalStop 禁充禁放（仅作为运行时初始值，EmuRuntime 首次计算前有效，代码中无主动赋值路径）
- 逻辑：`collector-engine/src/emu/emu_runtime.rs` 每秒根据当前SOC与 `charge_soc_limit`/`discharge_soc_limit` 比较得出：SOC ≥ 充电限值→禁充；SOC ≤ 放电限值→禁放；否则正常。当禁充时BCU电流为充电方向（<0）、或禁放时BCU电流为放电方向（>0），额外立即下发PCS有功功率0。
- 下发拦截：`power_guard.rs` 中 `EmuPolicy`（硬约束，最后钳位）读取 emu/2 号点位，对所有 `pcs_active_power` 下发做限制：禁充→功率上限0；禁放→功率下限0；TotalStop→强制0（读不到许可点位时同样按 TotalStop 处理）。
- 用途：控制/展示当前系统是否允许充电或放电，联动PCS功率下发实现SOC保护。

## 3. health_status（EMU告警故障状态）

- 类型：枚举 `u8`（`collector-core/src/runtime/emu.rs`，`HealthStatus`）
- 枚举值：
  - `0` Normal 正常
  - `1` Warning 告警
  - `2` Alarm 故障
- 逻辑：`collector-engine/src/emu/fault.rs` 中 `FaultDiagnosis` 策略每3秒扫描 pcs/bcu/tms 各故障寄存器展开出的告警位（见第12节 warn_fault），并合并单点遥信告警（见12.1），按 `WarnLevel` 聚合：命中 High → Warning；命中 Critical → Alarm（优先级最高）；均未命中 → Normal。
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
- 与运行模式（点位8）双向联动：使能开启即运行模式为计划自动，关闭即总功率（见第8节）。执行时仍同时检查两者，不满足则不下发，并清空"同一时段只下发一次"的记录，重新满足后（最长1分钟内）当前时段会重新下发。
- 逻辑：满足执行条件后（启动时及每分钟），从 SQLite 表 `t_plan_curve_master`/`t_plan_curve_detail` 取 `status=1` 且未删除的曲线，按 `priority`、创建时间排序，选第一条满足有效期/星期（以本地日期为准）的曲线；按15分钟粒度（`time_index` 0-95，对应00:00-23:45）读取该时段目标有功功率 `power_value`（正充负放，含0值时段）及可选 `soc_limit`。功率经逻辑字段 `pcs_active_power` 下发，同一 (曲线, 时段) 只下发一次。
- SOC限制判定（读 bcu/32）：`power_value>0`（充电）时 SOC ≥ `soc_limit`、`power_value<0`（放电）时 SOC ≤ `soc_limit`，则改为下发功率0；否则下发计划功率。
- 下发会经过功率保护拦截（见第11节），实际放行值可能被EMU许可/防逆流/需量保护钳位。
- 用途：控制是否启用"计划曲线"（分时功率计划）功能。

## 7. emu_power（EMU上电方式）

- 类型：命令型点位（整数，非常态上行状态量），实现于 `collector-engine/src/emu/cmd.rs`（`EmuPower` Command）
- 取值含义：
  - `1` 并网启动（grid_on_start）：启动bcu → 等待PCS通信建立 → 设置PCS远程 → 清除故障 → 开机
  - `2` 黑启动/离网启动（grid_off_start）：启动bcu → 等待PCS通信 → 设置远程/清除故障/VF离网模式/离网输出给定电压400V/开机命令
  - 其他值：无动作
- 用途：EMU系统整体上电启动方式选择（并网启动 vs 离网黑启动），一次性触发命令。

## 8. run_mode（EMU运行模式）

- 类型：枚举 `u8`（`collector-core/src/runtime/emu.rs`，`RunMode`）
- 枚举值：
  - `0` PlanAuto 计划自动
  - `1` TotalPower 总功率
- 逻辑：与计划曲线使能（点位6）双向联动，与控制源（点位9）无关：
  - 下发 `0`（计划自动）：自动开启计划曲线使能（持久化到 `t_emu_function`，并立即更新点位6）；下发 `1`（总功率）：自动关闭使能。
  - 反向：经点位6下发开启/关闭使能，运行模式随之变为计划自动/总功率。
  - 兜底：`emu_runtime.rs` 每秒以持久化的使能为准同步运行模式（使能开→计划自动，关→总功率），因此通过专用API等其他途径改使能也会跟随；重启后运行模式也据此恢复，不再固定回到总功率。
- 用途：选择EMU功率控制方式（跟随计划曲线自动下发 / 按总功率给定）。

## 9. control_source（EMU控制源）

- 类型：枚举 `u8`（`collector-core/src/runtime/emu.rs`，`ControlSource`）
- 枚举值：
  - `0` Local 本地
  - `1` Remote 远程
- 逻辑：下行写入切换，与运行模式无联动。控制源决定哪些通道可以下发点位（闸门在 EMU 启动时启用，未启用 EMU 的部署不受限制；判定在 `collector-core/src/runtime/emu.rs` 的 `allow_remote_dispatch` / `allow_api_dispatch`）：

| 通道 | 本地（0） | 远程（1） |
|---|---|---|
| API（`collector-api/src/services/data.rs` 的 `DataService::set`） | 允许 | 禁止，返回403；仅 emu 控制源点位（id 9）例外，否则无北向时无法切回本地 |
| 北向Modbus（`dock/modbus/server.rs`） | 忽略 | 允许 |
| 北向MQTT下发（`dock/mqtt/client.rs`） | 忽略 | 允许 |
| 脚本引擎 `dc.dispatch`（`mod_engine/api/dc.rs`） | 报错拒绝 | 允许 |

  - **运行模式（点位8）例外**：不论本地/远程，四个通道都允许修改运行模式（判定见 `collector-core/src/runtime/emu.rs` 的 `is_run_mode_point` / `is_run_mode_down`）。
  - API 的检查按整批请求判定，有任一非控制源点位、非运行模式点位即整体拒绝；MQTT 按点位过滤，本地时只放行其中的运行模式点位。
  - 只拦"点位下发"通道：`set_soc_protect`、`set_power_guard`、计划曲线使能等专用API不在此限。
  - 默认值为 `0`（本地），运行时状态不持久化，重启后恢复默认（此时北向与脚本引擎不能下发，需先经API切到远程）。
- 用途：选择EMU当前接受本地控制还是远程控制。

## 10. sys_tms_mode（系统热管理模式）

- 类型：枚举 `u8`（`collector-engine/src/emu/tms.rs`，`SysTmsMode`）
- 枚举值：
  - `0` Manual 手动
  - `1` Auto 自动
  - `2` Circulation 自循环
  - `3` Level1Cooling 一级制冷
  - `4` Level2Cooling 二级制冷
  - `5` Heating 制热
  - `6` Standby 待机
- 自动模式判定逻辑（每60秒执行一次，仅当当前模式为 Auto 时生效；启动时默认 Auto；基于bcu电池最高/最低/平均温度 `t_max`(bcu/19)/`t_min`(bcu/23)/`t_vag`(bcu/27)）：
  - BCU通讯断开（bcu/0xFFFF==1）或温度数据异常 → 回退一级制冷/22°C
  - `t_max∈[25,28)` 且 `t_vag∈[22,28)` → 自循环（仅水泵运行）
  - `t_min<=10` 且 `t_vag<=15` → 制热（出水温度15°C）
  - `t_max∈[28,34)` 且 `t_vag>=26` → 一级制冷（出水温度24°C）
  - `t_max>=34` 且 `t_vag>=28` → 二级制冷（出水温度22°C）
  - 否则 → 待机（关闭TMS使能）
- 下行控制：可下发本点位手动切换到以上任一模式。
- 底层对tms设备的实际动作寄存器：`id 2000` 热管理使能(bool)，`id 2001` 模式（Cooling=1/Heating=2/Circulation=3），`id 2002` 制冷出水温度设定，`id 2004` 制热出水温度设定。
- 用途：液冷机组（水冷系统）运行模式管理，依据电池温度自动切换制冷/制热/自循环/待机。

## 11. 防逆流 / 需量保护（点位11-16）

- 实现：`collector-engine/src/emu/guard_runtime.rs`（参数读写、回读、复评）+ `collector-engine/src/emu/power_guard.rs`（下发拦截与策略）+ `collector-core/src/runtime/emu.rs`（`PowerGuardConfig` 参数与校验）。
- 类型与单位：使能点（11、14）为 `Val::U8(0/1)`；其余为 `f64`，单位 kW。
- 默认值与校验（`PowerGuardConfig`）：

| 点位 | 默认 | 校验 |
|---|---|---|
| anti_backflow_enable | 0（关闭） | — |
| anti_backflow_threshold | 0.0（零逆流） | 有限且 ≥ 0 |
| anti_backflow_hysteresis | 2.0 | 有限且 > 0 |
| demand_guard_enable | 0（关闭） | — |
| demand_limit | 100.0 | 须 > 需量回差 |
| demand_hysteresis | 5.0 | 须 0 < 回差 < 需量限制 |

  校验以"当前值 + 修改值"整体进行，不通过则本次修改被拒绝并打警告日志；通过后落盘 `config/emu_runtime_config.json`（`power_guard` 段，与 `soc_protect` 同文件，兼容旧的仅SOC格式）。
- 采集/回读：功率保护策略每秒把6个参数写入 emu 设备；下行写入时按ID或key匹配并更新。
- 拦截机制：`PowerDispatchGuard` 注册为 DataCenter 的下发拦截器，仅拦截对 `pcs_active_power` 字段绑定点位的下发。各策略各自给出允许区间（`PowerLimit`），普通策略（防逆流、需量保护）取交集后钳位，硬约束（EMU充放电许可，见第2节）最后再钳位，日志标注生效的策略。
- 功率口径：下发值正充负放；关口功率取电为正、反送为负；PCS实际功率点位口径为负充正放，策略内取反后使用。预估关口功率 `free = P_grid + (P_请求 - P_pcs)`。
- 防逆流（`AntiBackflowPolicy`，限制放电）：`free < -阈值` 触发；生效期间功率下限为 `P_pcs - P_grid - 阈值 + 回差`；`free ≥ -阈值 + 回差` 释放。关口表或PCS功率读不到时保守禁放（下限0）。
- 需量保护（`DemandGuardPolicy`，限制充电/强制放电）：`free > 需量限制` 触发；生效期间功率上限为 `P_pcs - P_grid + 限制 - 回差`；`free ≤ 限制 - 回差` 释放。数据读不到时不限制（并复位状态）。目前按瞬时功率判断，15分钟滑窗需量预测未实现。
- 周期复评：上游可能只下发一次功率，负荷变化后无人再触发拦截，因此功率保护策略每秒用最近一次上游请求值重新求放行值，与上次放行值相差超过 0.5kW 死区才重新走一遍下发。
- 注意：`grid_active_power` 默认绑定为占位点位（grid_meter/9999），未改绑真实关口表点位时，开启防逆流会因数据无效而一直禁放；需量保护则不施加限制。

## 12. warn_fault（告警故障，点位范围500-2000）

代码中没有名为 `warn_fault` 的常量，该key是对500-2000这段ID区间的统称；实际每个具体故障位都有各自独立的key（取自设备Excel配置中告警位的英文名）。

- 动态生成逻辑：`collector-engine/src/emu/fault.rs`（`FaultDiagnosis` 策略，3秒一次）
  - 读取原始故障字寄存器：
    - pcs：id `[156,157,158,159,160,164,165]`（5个控制软件故障字 + 2个通讯软件故障字，各16bit）
    - bcu：id `[100..121]`（其他报警信息、Rack接触器状态、主控/从控故障、功能安全告警、均衡错误、BMU通信/概要故障、Rack严重/中度/轻微报警等，共22个寄存器×16bit）
    - tms：id `[20,21,22,23]`（故障代码1~4，各16bit）
  - 对每个寄存器逐位展开，过滤掉 `level==None` 的普通状态位，为每个命中的告警位生成一个新DataPoint（`key`=位的英文名，`name`=位的中文名，`value`=Val::U8(0/1)），统一分配 `id = 500 + 序号`。
  - 同时按位的 `WarnLevel` 联动更新 `health_status`。
  - 告警落库：命中的故障与 `t_alarm` 表中仍为 Active 的记录做差集（`alarm.rs`）：新增故障写入 Active 记录，此前Active但本次未命中的更新为 Recovered。打包位告警 `alarm_code = point.id * 16 + bit序号`，`alarm_dev` 为所属设备（pcs/bcu/tms）。库中级别映射：Critical→3 严重，High→2 重要，Normal/None→1 次要。
- 告警级别 `WarnLevel`（`collector-core/src/core/point.rs`）：
  - `0` None 非告警位（普通状态位）
  - `1` Normal 一般/轻微
  - `2` High 较严重（触发 health_status=Warning）
  - `3` Critical 严重故障（触发 health_status=Alarm）
- 告警位明细来源（非硬编码，来自Excel配置）：
  - 实际文件由 `config/config.json` 中各设备的 `register_file` 指定，下列文件名为当前仓库中的示例。
  - PCS：如 `config/pcs/PCS_英博_125.xlsx`（遥测sheet）。主要故障：A/B/C/N相硬件过流、辅助源掉电、绝缘故障、驱动故障、散热器/环境过温、输出过载过流、交流过压欠压/缺相/相序错误、直流反接、母线软件过压欠压、交流过频欠频、直流充放电过流、SPI通信故障、继电器合分闸故障、铁电参数保存错误、直流软起失败、电网孤岛故障、启动超时/条件不满足、支路/直路/交流不平衡、保险熔断、光纤通讯故障、电池软件过欠压、漏电流超限、零漂、ARM各类内部/EMS/BMS通讯故障、SD卡/RTC故障、急停信号、STS开关晶闸管过温等（绝大多数为级别2）。
  - BCU：如 `config/bms/BCU_永泰.xlsx`（信号sheet，CAN协议）。主要故障：从控概要故障、NTC故障、接触器粘连、内网通信故障、EEPROM故障、电流/绝缘检测故障、采样线/采样芯片/电压/温度采样故障、被动/主动均衡故障、极柱温度故障、总压差过大、单体电压/温度过大、SOC低、Rack级严重（level3）/中度（level2）/轻微（level1）报警（总压/单体过欠压、放充电电流/温度过大、绝缘阻值过低、压差温差过大等）。
  - TMS：如 `config/液冷机组/水冷机组_埃森特.xlsx`（遥测sheet）。主要故障：各类温度/压力传感器故障、系统高压告警、水泵故障、排气过热度低/压缩比异常、压缩机过流过温保护、水温/水压过高过低保护、滤网堵塞、水箱液位过低、风机1-4故障/离线、内存(E2)错误、驱动离线等（绝大多数为级别2）。
- 消费端：`collector-api/src/handlers/ws.rs`（`HomeWarnDatas::new`）调用 `center.read_range("emu", 500, 2000)`，过滤 `value==1` 的当前触发中告警，推送给前端首页告警列表。

### 12.1 单点遥信告警等级（`ModbusConfig.level`/`DataPoint.level`）

除了上面"打包位告警"（一个寄存器16个bit各自定义`zh|en|level`）外，还支持"单个遥信点位本身就是一个告警"的场景：

- 配置：Modbus点位表新增第20列（紧跟在重复次数/地址步长/序号步长之后）"告警等级"，取值 `0`/空=不告警，`1`/`2`/`3`=对应 `WarnLevel::Normal`/`High`/`Critical`。解析见 `collector-core/src/config/modbus_conf.rs`（`ModbusConfig.level`）。
- 运行时判定：`DataPoint::active_alarm_level()`（`collector-core/src/core/point.rs`）——配置了`level`且当前值非0即命中。
- 扫描逻辑：`collector-engine/src/emu/fault.rs` 的 `FaultDiagnosis::level_alarms` 每3秒遍历**所有设备**（`center.dev_ids()`）的**所有点位**，筛出命中的单点告警，与原有pcs/bcu/tms打包位告警合并为同一份 `warnings`，一起写入 `t_alarm` 表并联动 `health_status`。
- 与500-2000点位区间的区别：单点告警**不会**生成 emu 设备下的 `id=500+序号` 影子点位，只落库到 `t_alarm`、参与 `health_status` 聚合；`alarm_code` 直接用该点位自身的 `id`（因为同设备内id已在配置加载期校验唯一）。因此它不出现在 `read_range("emu",500,2000)` 的首页告警列表里，而是通过 `t_alarm` 表（`collector-api/src/routes/alarm.rs` 等告警相关接口）对外暴露。

---

## 13. grid_mode（并离网状态）

- 类型：枚举 `u8`，只读；由 PCS 两个遥信联合得出（`config/pcs/PCS_英博_125.xlsx` 遥信sheet）：
  - 1007 `gridConnectedState` 并网状态：1 为并网
  - 1008 `vfOffGridStat` VF离网状态：1 为离网
- 取值：

| 值 | 含义 | 1007 | 1008 |
|---|---|---|---|
| `0` | 非并网非离网（如停机/过渡中） | 0 | 0 |
| `1` | 并网 | 1 | 0 |
| `2` | 离网 | 0 | 1 |
| `3` | 状态冲突（异常） | 1 | 1 |

- 映射：逻辑字段 `pcs_grid_connected`（默认 pcs/1007）与 `pcs_off_grid`（默认 pcs/1008），可在字段绑定页改绑。`emu_runtime.rs` 每秒读取并写入 emu 设备 17 号点位。任一字段未绑定或读不到时不更新，沿用上一次的值（首次启动前无此点位）。
- 说明：本点位只读、不提供下行写入；切换并离网请用 emu_power（第7节，黑启动会向 pcs/2005 写 1 切到VF离网）。

---

## 相关文件一览

| 文件 | 作用 |
|---|---|
| `collector-engine/src/emu/mod.rs` | 各点位ID/KEY常量定义 |
| `collector-engine/src/emu/emu_runtime.rs` | operation_mode/permission/health_status/charge_soc_limit/discharge_soc_limit/run_mode/control_source 采集与写点逻辑 |
| `collector-engine/src/emu/planned_curve.rs` | planned_curve 点位与计划曲线执行逻辑 |
| `collector-engine/src/emu/cmd.rs` | emu_power 命令处理（并网/黑启动） |
| `collector-engine/src/emu/core.rs` | Emu 设备生命周期、策略/命令/字段/下发拦截器注册 |
| `collector-engine/src/emu/guard_runtime.rs` | 点位11-16 防逆流/需量保护参数读写与每秒复评 |
| `collector-engine/src/emu/power_guard.rs` | PowerDispatchGuard 有功功率下发拦截；EmuPolicy/AntiBackflowPolicy/DemandGuardPolicy |
| `collector-engine/src/emu/alarm.rs` | t_alarm 告警表读写（Active/Recovered、级别映射） |
| `collector-engine/src/emu/tms.rs` | sys_tms_mode 枚举、自动判定逻辑、下行控制 |
| `collector-engine/src/emu/fault.rs` | warn_fault(500-2000) 动态展开、health_status 联动 |
| `collector-engine/src/emu/taos.rs` | pcs/bcu/tms 原始寄存器列定义（用于TDengine落库） |
| `collector-core/src/runtime/emu.rs` | OperationMode/EmuPermission/HealthStatus/RunMode/ControlSource 枚举、RuntimeEmu（运行时状态+SOC保护/功率保护配置持久化）、PowerGuardConfig |
| `collector-core/src/runtime/planned_curve.rs` | RuntimePlannedCurve（计划曲线使能持久化，SQLite表 t_emu_function） |
| `collector-core/src/core/point.rs` | DataPoint/Bits/Bit/WarnLevel 通用定义、告警位解析 |
| `collector-api/src/handlers/ws.rs` | 首页告警列表API，读取 read_range("emu",500,2000) |
| `config/**/*.xlsx` | 实际故障码/告警位明细数据源（路径见 `config/config.json`） |
