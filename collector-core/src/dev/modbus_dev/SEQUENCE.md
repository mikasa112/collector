# Modbus 读写时序图

本文档描述 `collector-core/src/dev/modbus_dev` 当前的读写调度机制，
对应代码：`device.rs`（生命周期 / 下行 channel 注册）、`runner.rs`
（连接管理 + 统一 `tokio::select!` 主循环调度）、`block.rs`（分块读取 /
解析）、`downlink.rs`（写计划构建与下发）。

## 关键概念

| 概念 | 来源 | 含义 |
| --- | --- | --- |
| `timeout` | 协议配置 | 单次 Modbus 请求（读/写）的超时时间 |
| `request_interval` | 协议配置 | 写相关等待的固定间隔（`write_interval`，下限 1ms），**不随 block 数增长**；`WritePlan::apply` 内部每次实际写入之后都会等待一个该间隔 |
| `interval` | 协议配置 | 大循环轮询间隔；`== 0` 时读节拍间隔 `read_interval` 退化为 `write_interval`；`!= 0` 时按 block 数均分得到 `read_interval`（下限 1ms），使一整轮读取总耗时贴近配置值，避免随 block 数线性放大导致 CPU 占用过高 |
| `MAX_READ_FAILURES` | 常量 = 3 | 连续读取失败/超时达到该阈值即判定连接不可用，触发重连 |
| `ticker` | `time::interval(read_interval)` | 驱动读节拍的定时器，`MissedTickBehavior::Delay`：某次写入耗时较长导致错过 tick 时，不会在写完后瞬间“追帧”爆发式补读，而是顺延到下一个正常间隔 |

## 时序图

```mermaid
sequenceDiagram
    autonumber
    participant North as 北向下发者(北向应用/规则)
    participant Center as SharedPointCenter
    participant Dev as ModbusDev(生命周期)
    participant Runner as ModbusRunner
    participant Slave as Modbus 从站/网关

    North->>Dev: start()
    Dev->>Dev: 创建 mpsc::channel::<Vec<DownDataPoint>>(tx, rx)
    Dev->>Center: attach_downlink(id, tx)
    Dev->>Runner: tokio::spawn(runner.run())

    rect rgb(245,245,245)
    note over Runner: run()：连接 / 重连循环
    loop 直到收到停止信号
        Runner->>Slave: connect()（TCP connect 或 RTU 打开串口）
        alt 连接成功
            Runner->>Runner: backoff.reset()，状态 = Connected
            Runner->>Runner: run_connected() ⬇（见下方主循环）
        else 连接失败
            Runner->>Runner: 状态 = Failed，set_comm_fault(true)
            Runner->>Runner: sleep(backoff.next_delay()) 后重试
        end
    end
    end

    rect rgb(235,245,255)
    note over Runner,Slave: run_connected()：统一 tokio::select! 主循环（biased，每次只处理一个到位事件）
    Runner->>Runner: 计算 write_interval / read_interval，创建 ticker = interval(read_interval)（MissedTickBehavior::Delay）
    loop 每次循环仲裁一个事件（优先级：停止 > 写 > 读）
        par tokio::select! biased
            Dev->>Runner: stop_rx.changed()（任意时刻，最高优先级）
            Runner->>Runner: 收到停止 → set_comm_fault(true)，退出主循环
        and
            North->>Center: 下发写指令（任意时刻）
            Center->>Runner: 通过 tx 转发到 rx
            Runner->>Runner: rx.recv() 命中 → apply_write_batch()：构建 WritePlan
            Runner->>Slave: write_single/multiple_coils/registers
            Slave-->>Runner: 写入结果
            Runner->>Runner: WritePlan::apply 内部每次实际写入后等待 write_interval（无需额外等待）
            opt 写入失败 / rx 已关闭 / 写等待期间收到停止
                Runner->>Runner: set_comm_fault(true)，退出主循环（回重连循环）
            end
        and
            Runner->>Runner: ticker.tick() 到期（周期 = read_interval）
            Runner->>Slave: request_one(block[i])（timeout 超时保护，round-robin 游标推进）
            alt 读取成功
                Slave-->>Runner: BlockRead
                Runner->>Runner: fail_streak 清零
            else 读取失败或超时
                Runner->>Runner: fail_streak += 1
                opt fail_streak >= MAX_READ_FAILURES
                    Runner->>Runner: FailureThresholdReached，set_comm_fault(true)，退出主循环（回重连循环）
                end
            end
            alt 已读满一整圈 block
                Runner->>Runner: blocks.parse() 组装 DataPoint 列表
                Runner->>Center: ingest(id, entries)
                Center-->>North: 供上行订阅者/规则消费
            else 未读满一圈
                Runner->>Runner: Pending，等待下一个 tick 继续读下一个 block
            end
        end
    end
    end

    North->>Dev: stop()
    Dev->>Runner: stop_tx.send(true)
    Dev->>Center: detach_downlink(id)
    Runner->>Runner: 检测到 stop 信号，run() 退出
```

## 读写关键设计说明

1. **单一 `select!` 仲裁，读写共用同一条连接**：`run_connected` 用一个
   `biased` 的 `tokio::select!` 循环仲裁三路事件——停止信号、下行写命令
   （`rx.recv()`）、读节拍（`ticker.tick()`）。`biased` 保证优先级为
   停止 > 写 > 读，与原先“先排空写队列、再读一个 block”的语义等价，
   但去掉了手写的“先 try_recv 排空、再等待”状态机，逻辑更直观。
2. **写延迟与 block 数解耦**：写相关等待统一使用 `write_interval`
   （= `request_interval`，下限 1ms），不随 block 数量增长；`WritePlan::apply`
   内部每次实际写入之后都会等待一个 `write_interval`，无需在 `run_connected`
   里再额外等待。
3. **写命令不会被读节拍阻塞**：因为写命令走的是 `rx.recv()` 这一路
   `select!` 分支，一旦到达会立即被仲裁到并处理，不需要等 `ticker` 的下一次
   tick，写响应延迟因此始终 ≤ `write_interval`，与 `interval`/block 数无关。
4. **读间隔按需均分，用 `ticker` 驱动**：`interval == 0` 时 `read_interval`
   退化为 `write_interval`（等价于旧逻辑，仅防止忙等占满单核）；`interval != 0`
   时按 block 数均分为 `read_interval`，使一整轮读取的总耗时贴近配置值，
   而不是随 block 数线性放大，从而降低整体 CPU 占用。`ticker` 使用
   `MissedTickBehavior::Delay`：如果某次写入耗时较长错过了一个 tick，
   不会在写完后瞬间连续补读多次，而是顺延到下一个正常节拍。
5. **故障判定与重连**：单个 block 连续读取失败（含超时）达到
   `MAX_READ_FAILURES`（3 次），或写入失败、下行 channel 关闭，都会
   置位通讯故障并退出 `run_connected`，回到 `run()` 的指数退避重连循环。
