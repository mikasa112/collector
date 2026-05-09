# 统一优雅关闭方案

## 概述

本项目使用 `tokio-util` 的 `CancellationToken` 实现统一的优雅关闭机制，确保所有组件（API 服务器、设备管理器、MQTT 客户端等）能够协调一致地响应关闭信号。

## 架构设计

### 核心组件

**ShutdownManager** (`collector-core/src/shutdown.rs`)
- 封装 `CancellationToken`
- 监听系统信号（SIGTERM、SIGINT）
- 提供统一的关闭接口

### 优势

1. **类型安全**：编译时保证正确性
2. **层级取消**：支持父子令牌，父令牌取消时子令牌自动取消
3. **易于传递**：可克隆，无需 Arc 包装
4. **统一管理**：所有组件使用同一个关闭信号源

## 使用方法

### 1. 在主程序中创建 ShutdownManager

```rust
use collector_core::shutdown::ShutdownManager;

#[tokio::main]
async fn main() {
    // 创建统一的关闭管理器
    let shutdown = ShutdownManager::new();
    
    // 启动各个组件，传入 shutdown 或其令牌
    start_components(shutdown.clone()).await;
    
    // 在后台监听关闭信号
    tokio::spawn(shutdown.clone().listen_shutdown_signal());
    
    // 等待关闭信号
    shutdown.wait_for_shutdown().await;
    
    // 执行清理工作
    cleanup().await;
}
```

### 2. 在组件中使用取消令牌

#### 方式 A：传递 ShutdownManager

```rust
pub async fn start_server(shutdown: ShutdownManager) {
    let server = create_server();
    
    tokio::spawn(async move {
        shutdown.wait_for_shutdown().await;
        server.stop_graceful().await;
    });
    
    server.run().await;
}
```

#### 方式 B：传递 CancellationToken

```rust
use tokio_util::sync::CancellationToken;

pub async fn background_task(token: CancellationToken) {
    loop {
        tokio::select! {
            _ = token.cancelled() => {
                println!("任务收到关闭信号");
                break;
            }
            _ = do_work() => {
                // 继续工作
            }
        }
    }
}
```

### 3. 创建子令牌（可选）

```rust
// 父令牌取消时，子令牌自动取消
let child_token = shutdown.child_token();

tokio::spawn(async move {
    child_task(child_token).await;
});
```

## 实际应用示例

### collector-cmd 主程序

```rust
pub async fn cmd() {
    // 创建关闭管理器
    let shutdown = ShutdownManager::new();
    
    // 启动所有组件
    let mut manager = DevManager::new(devices, center);
    manager.start_all().await;
    
    // 后台监听关闭信号
    tokio::spawn(shutdown.clone().listen_shutdown_signal());
    
    // 等待关闭信号
    shutdown.wait_for_shutdown().await;
    
    // 优雅关闭所有组件
    manager.stop_all().await;
}
```

### collector-api 服务器

```rust
pub async fn start(&self, shutdown: ShutdownManager) {
    let server = Server::new(acceptor);
    let handle = server.handle();
    
    tokio::spawn(async move {
        shutdown.wait_for_shutdown().await;
        handle.stop_graceful(None);
    });
    
    server.serve(router).await;
}
```

## 关闭流程

1. **接收信号**：用户按 Ctrl+C 或系统发送 SIGTERM
2. **触发取消**：`ShutdownManager` 取消 `CancellationToken`
3. **通知组件**：所有等待 `cancelled()` 的任务收到通知
4. **优雅关闭**：
   - API 服务器停止接受新请求，等待现有请求完成
   - 设备管理器停止所有设备任务
   - MQTT 客户端断开连接
5. **退出程序**：所有清理工作完成后退出

## 最佳实践

### ✅ 推荐做法

```rust
// 1. 使用 tokio::select! 响应取消信号
tokio::select! {
    _ = token.cancelled() => break,
    result = work() => handle(result),
}

// 2. 在长时间运行的循环中定期检查
loop {
    if token.is_cancelled() {
        break;
    }
    do_work().await;
}

// 3. 传递子令牌给独立任务
let child = shutdown.child_token();
tokio::spawn(async move {
    task(child).await;
});
```

### ❌ 避免做法

```rust
// 不要忽略取消信号
loop {
    do_work().await; // 永远不会停止
}

// 不要在取消后继续工作
if token.is_cancelled() {
    do_more_work().await; // 错误！
}
```

## 测试

### 手动测试

```bash
# 启动程序
cargo run

# 按 Ctrl+C 触发优雅关闭
# 观察日志输出，确认所有组件正确关闭
```

### 发送 SIGTERM（Unix）

```bash
# 获取进程 ID
ps aux | grep collector

# 发送 SIGTERM
kill -TERM <PID>
```

## 故障排查

### 问题：程序不响应 Ctrl+C

**可能原因**：
- 某个任务阻塞了 tokio 运行时
- 没有正确传递取消令牌

**解决方案**：
- 检查所有长时间运行的任务是否使用了 `tokio::select!`
- 确保所有组件都接收并响应取消信号

### 问题：关闭时间过长

**可能原因**：
- 某些任务没有及时响应取消信号
- 清理工作耗时过长

**解决方案**：
- 为 `stop_graceful()` 设置超时时间
- 优化清理逻辑，移除不必要的等待

## 相关资源

- [tokio-util CancellationToken 文档](https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html)
- [Tokio 优雅关闭指南](https://tokio.rs/tokio/topics/shutdown)
