# 自定义 TCP 通讯协议文档（V1.0）

## 1. 目标
用于设备与平台之间传输点值数据（Data）、控制命令（Control）及字典数据，支持校验、可扩展、可演进。

## 2. 传输层
1. 基于 TCP 长连接
2. 字节序：大端（Big-Endian）
3. 每个 TCP 包可承载 1~N 个完整帧；接收端必须按流拆包粘包处理

## 3. 帧总览
一帧由三部分组成：
1. Header（固定+可选扩展）
2. Payload（业务负载）
3. CRC32（4 字节）

总长度：
`FrameLen = HeaderLen + PayloadLen + 4`

## 4. Header 字段定义（固定头）
| 字段 | 类型 | 长度(字节) | 说明 |
|---|---:|---:|---|
| magic | u16 | 2 | 固定 `0xCC01` |
| version | u8 | 1 | 协议版本，当前 `0x01` |
| msg_type | u8 | 1 | 消息类型，见第5节 |
| flags | u16 | 2 | 位标志，见第6节 |
| header_len | u16 | 2 | 头总长度（含扩展头），最小 26 |
| payload_len | u32 | 4 | 负载长度 |
| seq | u32 | 4 | 发送序号，按发送方向单向递增（请求与响应各自独立） |
| timestamp_ms | u64 | 8 | Unix 毫秒时间戳 |
| device_id_len | u16 | 2 | 设备ID长度（字节） |

固定头最小长度：26 字节。  
若 `header_len > 26`，后续为扩展头 TLV（可选）。

## 5. MsgType 定义
| 值 | 名称 | 含义 |
|---:|---|---|
| 1 | Data | 点值数据上送（YC/YX/YK/YT） |
| 2 | Control | 控制命令下发（YK/YT） |
| 5 | Dict | 字典/点表同步 |
| 6 | Heartbeat | 心跳 |
| 7 | Ack | 确认应答 |
| 255 | Error | 错误应答 |

## 6. Flags 位定义（u16）
| 位 | 名称 | 说明 |
|---:|---|---|
| bit0 | ACK_REQUIRED | 1=需要对端 Ack |
| bit1 | IS_RESPONSE | 1=响应帧 |
| bit2 | IS_FRAGMENT | 1=分片帧 |
| bit3 | IS_COMPRESSED | 1=Payload已压缩（zstd） |
| bit4~15 | 保留 | 必须置0 |

## 7. Payload 通用前缀
所有业务消息 Payload 开头统一带：
1. `device_id[device_id_len]`（UTF-8）
2. `body[...]`（按消息类型定义）

## 8. 各消息体定义

### 8.1 Data
`body` 结构：
1. `point_count: u16`
2. 重复 `point_count` 次：
   - `point_id: u32`
   - `domain: u8`（0=UNKNOWN,1=YK,2=YX,3=YT,4=YC）
   - `value_type: u8`
   - `value_len: u16`
   - `value[value_len]`

`value_type`：
1=U8, 2=I8, 3=I16, 4=I32, 5=U16, 6=U32, 7=F32, 8=BOOL, 9=UTF8_STRING

### 8.2 Control
`body` 结构：
1. `cmd_id: u32`
2. `point_id: u32`
3. `value_type: u8`
4. `value_len: u16`
5. `value[value_len]`
6. `timeout_ms: u32`（可选，置0表示默认）

会话约束（当前实现）：
1. 服务端在单个 TCP 会话内会将首个 Control 的 `device_id` 绑定为会话目标设备
2. 后续 Control 若 `device_id` 不一致，服务端返回确认失败（Ack `code=1006`）并拒绝下发

### 8.3 Dict
`body` 结构：
1. `dict_version: u32`
2. `entry_count: u16`
3. 重复 `entry_count` 次：
   - `point_id: u32`
   - `name_len: u16`
   - `name[name_len]`（UTF-8）
   - `unit_len: u8`
   - `unit[unit_len]`（UTF-8，可空）
   - `value_type: u8`

连接建立后，服务端下发 Dict 帧时应设置 `ACK_REQUIRED=1`，客户端需返回 Ack。
若未在超时时间内收到 Ack，服务端可按配置重发，超过最大重试次数后断开连接。

### 8.4 Heartbeat
`body` 可为空，推荐带：
1. `status: u8`（0=init,1=ready,2=running,3=degraded）
2. `uptime_s: u32`

### 8.5 Ack
`body` 结构：
1. `ack_seq: u32`（被确认帧序号）
2. `code: u16`（0=OK）
3. `msg_len: u8`
4. `msg[msg_len]`

说明：
1. Ack 帧自身 `header.seq` 为发送方当前会话出站序号
2. 被确认的序号放在 `body.ack_seq`

### 8.6 Error
`body` 结构：
1. `err_code: u16`
2. `ref_seq: u32`
3. `msg_len: u8`
4. `msg[msg_len]`

说明：
1. Error 帧自身 `header.seq` 为发送方当前会话出站序号
2. 被引用的请求序号放在 `body.ref_seq`

## 9. CRC 校验
1. CRC 类型：CRC32 IEEE
2. 计算范围：从 `magic` 起到 `payload` 末尾止
3. CRC 字段位置：帧尾 4 字节（大端）

## 10. 接收端处理流程
1. 先找 `magic`
2. 缓冲长度不足固定头（26）则继续收
3. 读出 `header_len/payload_len`，检查上限
4. 缓冲不足 `header_len + payload_len + 4` 则继续收
5. 做 CRC32 校验，不通过丢弃并回 Error
6. 按 `msg_type` 分发处理

## 11. 约束与限制（建议）
1. `payload_len` 最大建议 1MB（可配置）
2. `device_id_len` 最大 128
3. `point_count` 单帧最大 5000
4. 超限返回 `Error(code=1002)`

## 12. 错误码建议
| 码值 | 含义 |
|---:|---|
| 0 | OK |
| 1001 | BAD_MAGIC |
| 1002 | LENGTH_OUT_OF_RANGE |
| 1003 | CRC_MISMATCH |
| 1004 | UNSUPPORTED_VERSION |
| 1005 | UNSUPPORTED_MSG_TYPE |
| 1006 | INVALID_PAYLOAD |
| 1007 | INTERNAL_ERROR |

## 13. 版本演进规则
1. 新增字段优先放扩展头或 body 尾部
2. 未识别的扩展 TLV 必须跳过
3. 破坏性变更必须升级 `version`

## 14. 扩展头 TLV（可选）
当 `header_len > 26`，附加 TLV：
1. `type: u8`
2. `len: u8`
3. `value[len]`

建议 TLV：
1. `type=1` trace_id
2. `type=2` tenant_id
3. `type=3` sign/hmac
