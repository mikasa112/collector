use collector_core::shutdown::ShutdownManager;
use tokio_util::sync::CancellationToken;

mod core;
pub mod mod_engine;
pub mod strategy;

pub use core::FaultDiagnosis;
pub use strategy::{Schedule, Strategy};

pub struct Engine {
    strategies: Vec<Box<dyn Strategy + Send>>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self { strategies: vec![] }
    }

    pub fn register<S: Strategy>(mut self, strategy: S) -> Self {
        self.strategies.push(Box::new(strategy));
        self
    }

    pub async fn start(self, shutdown: ShutdownManager) {
        let handles: Vec<_> = self
            .strategies
            .into_iter()
            .map(|s| {
                let token = shutdown.child_token();
                tokio::spawn(run_strategy(s, token))
            })
            .collect();

        shutdown.wait_for_shutdown().await;
        tracing::info!("策略引擎正在关闭...");

        for h in handles {
            let _ = h.await;
        }
    }
}

async fn run_strategy(mut strategy: Box<dyn Strategy + Send>, token: CancellationToken) {
    let name = strategy.name().to_owned();
    let schedule = strategy.schedule();

    tracing::info!("[策略] {} 启动", name);

    if let Err(e) = strategy.on_start().await {
        tracing::error!("[策略] {} 启动失败: {}", name, e);
        return;
    }

    match schedule {
        Schedule::Interval(dur) => {
            let mut ticker = tokio::time::interval(dur);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = ticker.tick() => {
                        if let Err(e) = strategy.on_tick().await {
                            tracing::error!("[策略] {} 执行出错: {}", name, e);
                        }
                    }
                }
            }
        }
        Schedule::Cron(expr) => {
            let cron_sched: cron::Schedule = match expr.parse() {
                Ok(s) => s,
                Err(e) => {
                    tracing::error!("[策略] {} cron 解析失败: {}", name, e);
                    return;
                }
            };
            while let Some(next) = cron_sched.upcoming(chrono::Utc).next() {
                let delay = (next - chrono::Utc::now()).to_std().unwrap_or_default();
                tokio::select! {
                    _ = token.cancelled() => break,
                    _ = tokio::time::sleep(delay) => {
                        if let Err(e) = strategy.on_tick().await {
                            tracing::error!("[策略] {} 执行出错: {}", name, e);
                        }
                    }
                }
            }
        }
    }

    tracing::info!("[策略] {} 已停止", name);
}
