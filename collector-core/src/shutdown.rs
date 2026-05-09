use tokio::signal;
use tokio_util::sync::CancellationToken;
use tracing::info;

/// 统一的优雅关闭管理器
#[derive(Clone)]
pub struct ShutdownManager {
    token: CancellationToken,
}

impl ShutdownManager {
    /// 创建新的关闭管理器
    pub fn new() -> Self {
        Self {
            token: CancellationToken::new(),
        }
    }

    /// 获取取消令牌（用于传递给子任务）
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }

    /// 创建子令牌（父令牌取消时，子令牌自动取消）
    pub fn child_token(&self) -> CancellationToken {
        self.token.child_token()
    }

    /// 监听系统关闭信号并触发取消
    pub async fn listen_shutdown_signal(self) {
        let ctrl_c = async {
            signal::ctrl_c()
                .await
                .expect("failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            signal::unix::signal(signal::unix::SignalKind::terminate())
                .expect("failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => {
                info!("收到 Ctrl+C 信号，开始优雅关闭...");
            }
            _ = terminate => {
                info!("收到 SIGTERM 信号，开始优雅关闭...");
            }
        }

        self.token.cancel();
    }

    /// 等待关闭信号
    pub async fn wait_for_shutdown(&self) {
        self.token.cancelled().await;
    }

    /// 检查是否已收到关闭信号
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }
}

impl Default for ShutdownManager {
    fn default() -> Self {
        Self::new()
    }
}
