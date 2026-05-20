use collector_core::shutdown::ShutdownManager;

mod action;
mod core;

pub struct Engine {}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn start(self, shutdown: ShutdownManager) {
        tokio::spawn(async move {
            shutdown.wait_for_shutdown().await;
            tracing::info!("策略引擎正在关闭...");
        });
    }
}
