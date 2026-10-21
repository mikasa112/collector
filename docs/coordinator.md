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
  "members": [
    { "dev": "ctrl_a", "rated_kva": 500.0,
      "p_set": 1, "q_set": 2, "p_actual": 3, "q_actual": 4,
      "p_upper": 5, "p_lower": 6, "q_upper": 7, "q_lower": 8 }
  ]
}
```

| 字段 | 说明 |
|---|---|
| `id` | 虚拟设备 id，北向点表的 `source` 填它。 |
| `tick_ms` | 聚合、分配周期。收到设定后会立即重算，不必等下一个周期。 |
| `upstream_timeout_ms` | 远方且已投入时，北向连接全部断开超过该时间，总设定值清零。`0` 不启用。北向子站没运行时不判断。 |
| `p_deadband` / `q_deadband` | 新分配值与上次下发值之差小于它则不重复下发（kW / kvar）。降到 0 时不受死区限制。 |
| `members[].dev` | 下级设备在 DataCenter 里的 id。 |
| `members[].rated_kva` | 额定视在容量，既是分配权重，也用于 `P²+Q²≤S²` 约束。 |
| `members[].p_set` `q_set` | 下发有功、无功设定值的点 id。 |
| `members[].p_actual` `q_actual` | 实测有功、无功的点 id。 |
| `members[].p_upper` `p_lower` `q_upper` `q_lower` | 下级上送的有功、无功上下限点 id。 |

设定值、实测值、限值**必须使用同一符号约定**，且与北向一致，采集器不做符号转换。

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

1. 每台成员的可调范围 = 下级上送的上下限，再受额定容量约束：有功限在 `±S`，无功限在 `±√(S²−P²)`。无功约束用到该成员分到的有功，所以先分有功，再分无功。
2. 目标值先截断到在线成员总可调范围内，被截断的部分上送为"未满足"。
3. 按 `rated_kva` 比例分配。某台触及上下限就钳在边界，剩余量在其余成员间继续按比例分配，直到分完。
4. 离线成员不参与：通讯状态点（65535）非 0，或四个限值点还没上送，或限值异常（下限大于上限）。它的限值不计入总可调范围，也不向它下发。
5. 设定值保持：每个周期重新计算。某台离线又恢复、限值变化时会自动重分配，不依赖上级重发。

## 已知限制

- 没有调节速率限制，只有死区。
- 不支持按 SOC 等策略分配，目前只按额定容量比例。
- 本地 API 下发（HTTP）不受远方/就地限制，只有北向受门禁约束。
- 模式和设定值不持久化，重启后回到初始状态。
- 成员的设定、实测、限值点依赖点 id 配置，配置错误时该成员表现为离线或下发无效，注意核对日志。
