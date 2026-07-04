use collector_core::{center::SharedPointCenter, shutdown::ShutdownManager};
use tokio_util::sync::CancellationToken;

use crate::{
    emu::{
        cmd::{self, Command},
        tms,
    },
    strategy::{Schedule, Strategy},
};

/**
 * 命令:
 *  1. 根据点做动作的命令
 *
 * 策略:
 *  1.无映射到点的策略
 *  2.有映射到点的策略
 */
pub struct Emu {
    pub commands: Vec<Box<dyn Command>>,
    strategies: Vec<Box<dyn Strategy>>,
}

impl Emu {
    pub async fn new(center: SharedPointCenter) -> Self {
        let commands: Vec<Box<dyn Command>> = vec![Box::new(cmd::PowerOn)];
        let strategies: Vec<Box<dyn Strategy>> = vec![Box::new(tms::Tms::new(center.clone()))];
        Self {
            commands,
            strategies,
        }
    }

    pub fn register<S: Strategy>(mut self, strategy: S) -> Self {
        self.strategies.push(Box::new(strategy));
        self
    }

    pub async fn run(self, shutdown: ShutdownManager) {
        let strategy_handles: Vec<_> = self
            .strategies
            .into_iter()
            .map(|s| {
                let token = shutdown.child_token();
                tokio::spawn(run_strategy(s, token))
            })
            .collect();
        shutdown.wait_for_shutdown().await;
        tracing::info!("策略引擎正在关闭...");
        for h in strategy_handles {
            let _ = h.await;
        }
    }
}

async fn run_strategy(mut strategy: Box<dyn Strategy + Send>, token: CancellationToken) {
    let name = strategy.name().to_owned();
    let schedule = strategy.schedule();
    tracing::info!("[{}策略] 启动", name);
    if let Err(e) = strategy.on_start().await {
        tracing::error!("[{}策略] 启动失败: {}", name, e);
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
                            tracing::error!("[{}策略] 执行出错: {}", name, e);
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
                            tracing::error!("[{}策略] 执行出错: {}", name, e);
                        }
                    }
                }
            }
        }
    }
    tracing::info!("[策略] {} 已停止", name);
}
