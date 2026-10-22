# 协调控制器（AGC / AVC）

实现：`collector-engine/src/coordinator/`（`alloc.rs` 分配算法、`logic.rs` 状态机、`core.rs` 虚拟设备）
北向接入：`collector-core/src/dock/iec104/`，通过 `NorthboundGate`（`collector-core/src/dispatch.rs`）拒绝命令

多台下级控制器（如两台 IEC 104 协调控制器 A、B）接入采集器后，合并成**一个虚拟设备**。
北向只面对这个虚拟设备：上级下发总有功、总无功设定值，采集器分配给 A、B；上级读到的是 A+B 的合并量。

```
上级主站 ─遥调→ 北向104 ─门禁→ 协调控制器(agc) ─分配→ ctrl_a / ctrl_b
上级主站 ←突发─ 北向104 ←聚合─ 协调控制器(agc) ←订阅─ ctrl_a / ctrl_b
```

## 启用（config.json 的 `program.coordinator`）

```json
"coordinator": {
  "enable": true,
  "id": "agc",
  "tick_ms": 500,
  "upstream_timeout_ms": 60000,
  "p_deadband": 1.0,
  "q_deadband": 1.0,
  "positive_is_discharge": true,
  "members": [
    { "dev": "ctrl_a", "rated_kva": 500.0,
      "p_set": 1, "q_set": 2, "p_actual": 3, "q_actual": 4,
      "p_max_charge": 5, "p_max_discharge": 6 }
  ]
}
```

| 字段 | 说明 |
|---|---|
| `id` | 虚拟设备 id，北向点表的 `source` 填它。 |
| `tick_ms` | 聚合、分配周期。收到设定后会立即重算，不必等下一个周期。 |
| `upstream_timeout_ms` | 远方且已投入时，北向连接全部断开超过该时间，总设定值清零。`0` 不启用。北向子站没运行时不判断。 |
| `positive_is_discharge` | **必填，没有默认值**：`true` 正放负充，`false` 正充负放。设定值、实测值都按它，必须与北向上级约定的方向一致；没填时协调控制器拒绝启动。 |
| `p_deadband` / `q_deadband` | 新分配值与上次下发值之差小于它则不重复下发（kW / kvar）。降到 0 时不受死区限制。 |
| `members[].dev` | 下级设备在 DataCenter 里的 id。 |
| `members[].rated_kva` | 额定视在容量，既是分配权重，也用于 `P²+Q²≤S²` 约束。 |
| `members[].p_set` `q_set` | 下发有功、无功设定值的点 id。 |
| `members[].p_actual` `q_actual` | 实测有功、无功的点 id。 |
| `members[].p_max_charge` | 设备上送的**最大可充电功率**点 id（正数，瞬时值）。 |
| `members[].p_max_discharge` | 设备上送的**最大可放电功率**点 id（正数，瞬时值）。 |
| `members[].q_upper` `q_lower` | 选填：设备上送的无功上下限点 id（带符号）。设备没有这两个点时不填，无功只按额定容量约束；必须同时填或同时不填。 |

设定值、实测值**必须使用同一符号约定**（即 `positive_is_discharge`），且与北向一致，采集器不做设定值的符号转换。

## 在项目配置 xlsx 里配置

`config.json` 由项目配置 xlsx 经 `python3 scripts/export_config_from_xlsm.py <xlsx> [输出json]` 导出，不用手写 JSON。

- **标量字段**写在 `program` sheet：`feature` 填 `COORDINATOR`，`enable` 列填 `1`/`0`，`param`/`value` 填 `id`、`tick_ms`、`upstream_timeout_ms`、`p_deadband`、`q_deadband`。
- **成员**写在单独的 `coordinator_members` sheet（也认 `协调控制器成员`），第 1 行是表头，**一行一台**：

| dev | rated_kva | p_set | q_set | p_actual | q_actual | p_max_charge | p_max_discharge | q_upper | q_lower | enable |
|---|---|---|---|---|---|---|---|---|---|---|
| ctrl_a | 500 | 1 | 2 | 3 | 4 | 5 | 6 | | | |
| ctrl_b | 500 | 1 | 2 | 3 | 4 | 5 | 6 | | | |

列含义同下方字段表；`q_upper`、`q_lower` 可以留空（要么都填，要么都空），`enable` 可省略，填 `0` 的行跳过。导出时会校验：必填列不能为空，点位 id 必须是非负整数，`rated_kva` 必须大于 0，设备 id 不能重复。
- `positive_is_discharge` 写在 `program` sheet 的 `COORDINATOR` 下，填 `true`/`false`（或 `1`/`0`）。启用协调控制器时不填，导出会直接报错。
- **不要**在 `program` sheet 里写 `members` 这一行：单元格里的 JSON 文本会变成字符串，采集器解析 `config.json` 会失败。导出脚本遇到这种情况，有成员表就以成员表为准，没有就直接报错。

## 协调控制器的点位

北向点表（`iec104_server`）里 `source` 填 `id`，`point_id` 填下表编号。1~6 可读可写，写入后回读即为回显，所以同一个点可以同时配监视点和控制点（共用 `ca/ioa`）。

| point_id | key | 含义 | 北向 type |
|---|---|---|---|
| 1 | agc_p_set | 总有功设定（回显） | `yt_short` + `yc_short` |
| 2 | avc_q_set | 总无功设定（回显） | `yt_short` + `yc_short` |
| 3 | agc_enable | AGC 投入（1 投入 / 0 退出） | `yk` + `yx` |
| 4 | avc_enable | AVC 投入 | `yk` + `yx` |
| 5 | agc_remote | AGC 远方（1 远方 / 0 就地） | `yk` + `yx` |
| 6 | avc_remote | AVC 远方 | `yk` + `yx` |
| 10 / 11 | total_p / total_q | 总有功、总无功（在线成员实测之和） | `yc_short` |
| 12 / 13 | p_upper / p_lower | 有功可调上限、下限 | `yc_short` |
| 14 / 15 | q_upper / q_lower | 无功可调上限、下限 | `yc_short` |
| 16 / 17 | agc_inc_block / agc_dec_block | AGC 增、减闭锁（目标已到边界） | `yx` |
| 18 / 19 | avc_inc_block / avc_dec_block | AVC 增、减闭锁 | `yx` |
| 20 / 21 | agc_unmet / avc_unmet | 未满足的有功、无功（被截断的部分） | `yc_short` |
| 65535 | communicationStatus | 任一成员离线为 1 | `yx` |

## 模式

| 状态 | 北向总设定值 | 开关（投入/退出、远方/就地） | 分配 |
|---|---|---|---|
| 远方 + 投入 | 接受 | 可写 | 进行 |
| 就地 + 投入 | **否定确认** | 可写 | 进行（设定值来自本地） |
| 退出 | **否定确认** | 可写 | 不进行 |

- AGC（有功）与 AVC（无功）各有一套开关，互不影响。
- 启动时：AGC、AVC 都是**退出**、**远方**，设定值为 0。重启不恢复上次的设定。
- **投入**：设定值从 0 开始，各成员先收到一次 0。
- **退出**：设定值回到 0，各成员收到 0。退出时离线的成员，在恢复在线后补发一次 0。之后不再干预，由成员自己或本地操作。
- 北向遥调在门禁处被拒绝，上级收到的是否定确认（ACT_CON 的 negative 位）。本地 API 下发不经过门禁。
- **上级失联**：仅在远方且已投入时生效。清零后一直保持 0，直到上级重新下发。

## 分配

1. 每台成员的有功范围由它上送的最大可充、最大可放换算（随 SOC、温度实时变化），再受额定容量约束限在 `±S`：
   - 正放负充：`[-最大可充, 最大可放]`；正充负放：`[-最大可放, 最大可充]`。
   - 无功范围限在 `±√(S²−P²)`，设备上送了无功上下限则再收紧；没有就只按容量约束。无功约束用到该成员分到的有功，所以先分有功，再分无功。
2. 目标值先截断到在线成员总可调范围内，被截断的部分上送为"未满足"。
3. 按 `rated_kva` 比例分配。某台触及上下限就钳在边界，剩余量在其余成员间继续按比例分配，直到分完。
4. 离线成员不参与：通讯状态点（65535）非 0，或最大可充/可放点还没上送、数值异常（非数字或负数），或已配置的无功限值点还没上送。它的限值不计入总可调范围，也不向它下发。
5. 设定值保持：每个周期重新计算。某台离线又恢复、限值变化时会自动重分配，不依赖上级重发。

## 已知限制

- 没有调节速率限制，只有死区。
- 不支持按 SOC 等策略分配，目前只按额定容量比例。
- 本地 API 下发（HTTP）不受远方/就地限制，只有北向受门禁约束。
- 模式和设定值不持久化，重启后回到初始状态。
- 成员的设定、实测、最大可充/可放点依赖点 id 配置，配置错误时该成员表现为离线或下发无效，注意核对日志。
- 最大可充/可放必须是**正数**；设备上送负数会被当成数据异常，该成员按离线处理。
