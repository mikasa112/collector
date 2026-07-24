use std::{sync::Arc, time::Duration};

use crate::{
    emu::{
        cmd::{self, Command},
        emu_runtime, fault, planned_curve, tms,
    },
    strategy::{Schedule, Strategy},
};
use collector_core::{
    center::{DataCenterError, SharedPointCenter},
    core::point::DownDataPoint,
    dev::{DeviceError, Executable, Identifiable, Lifecycle, LifecycleState, state::SharedState},
    utils::database::get_database,
};
use parking_lot::Mutex;
use tokio::{
    sync::{Mutex as AsyncMutex, watch},
    task::JoinHandle,
    time,
};

pub struct Emu {
    id: String,
    state: SharedState,
    pub commands: Arc<AsyncMutex<Vec<Box<dyn Command>>>>,
    strategies: Arc<AsyncMutex<Vec<Box<dyn Strategy>>>>,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
    handles: Mutex<Vec<JoinHandle<()>>>,
    center: SharedPointCenter,
}

impl Emu {
    pub async fn new(center: SharedPointCenter) -> Self {
        let commands: Arc<AsyncMutex<Vec<Box<dyn Command>>>> =
            Arc::new(AsyncMutex::new(vec![Box::new(cmd::PowerOn)]));
        let pool = get_database().expect("[engine] 数据库初始化失败");
        let strategies: Arc<AsyncMutex<Vec<Box<dyn Strategy>>>> = Arc::new(AsyncMutex::new(vec![
            Box::new(emu_runtime::EmuRuntime::new(center.clone())),
            Box::new(fault::FaultDiagnosis::new(center.clone())),
            Box::new(tms::Tms::new(center.clone())),
            Box::new(planned_curve::PlannedCurve::new(center.clone(), pool)),
        ]));
        let state = SharedState::new(LifecycleState::New);
        let (stop_tx, stop_rx) = watch::channel(false);
        Self {
            id: String::from("emu"),
            state,
            commands,
            strategies,
            stop_tx,
            stop_rx,
            handles: Mutex::new(Vec::new()),
            center,
        }
    }

    fn load_state(&self) -> LifecycleState {
        self.state.load()
    }

    fn cas_state(&self, from: LifecycleState, to: LifecycleState) -> bool {
        self.state.cas(from, to)
    }

    fn store_state(&self, to: LifecycleState) {
        self.state.store(&self.id, to);
    }
}

#[async_trait::async_trait]
impl Lifecycle for Emu {
    fn init(&self) -> Result<(), DeviceError> {
        if !self.cas_state(LifecycleState::New, LifecycleState::Initializing) {
            return Ok(());
        }
        self.store_state(LifecycleState::Ready);
        Ok(())
    }

    async fn start(&mut self) -> Result<(), DeviceError> {
        //尝试将状态改为启动中，如果失败则不动作
        let ok = self.cas_state(LifecycleState::Ready, LifecycleState::Starting)
            || self.cas_state(LifecycleState::Stopped, LifecycleState::Starting);
        if !ok {
            return Ok(());
        }
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<DownDataPoint>>(16);
        //将设备注册到消息中心
        match self.center.attach_downlink(&self.id, tx.clone()) {
            Ok(()) => {}
            Err(DataCenterError::DevHasRegister(_)) => {
                self.center.detach_downlink(&self.id);
                if let Err(err) = self.center.attach_downlink(&self.id, tx) {
                    tracing::warn!("[{}] 重新注册数据中心失败: {}", self.id, err);
                    self.store_state(LifecycleState::Failed);
                    return Ok(());
                }
            }
            Err(err) => {
                tracing::warn!("[{}] 重新注册数据中心失败: {}", self.id, err);
                self.store_state(LifecycleState::Failed);
                return Ok(());
            }
        }
        //复位停止信号，避免上一次 stop() 的信号影响本次启动
        let _ = self.stop_tx.send(false);
        //策略保留在共享容器中（而非取出所有权），这样下行点位才能通过同一个容器路由到策略
        let strategy_count = self.strategies.lock().await.len();
        let mut handles: Vec<_> = (0..strategy_count)
            .map(|idx| {
                let strategies = self.strategies.clone();
                let stop_rx = self.stop_rx.clone();
                tokio::spawn(run_strategy(strategies, idx, stop_rx))
            })
            .collect();
        let commands_clone = self.commands.clone();
        let strategies_clone = self.strategies.clone();
        handles.push(tokio::spawn(run_downlink(
            rx,
            commands_clone,
            strategies_clone,
            self.stop_rx.clone(),
        )));
        *self.handles.lock() = handles;
        self.store_state(LifecycleState::Running);
        Ok(())
    }

    async fn stop(&self) -> Result<(), DeviceError> {
        let _ = self.stop_tx.send(true);
        let cur = self.load_state();
        match cur {
            LifecycleState::Stopped => return Ok(()),
            LifecycleState::New | LifecycleState::Ready => {
                self.store_state(LifecycleState::Stopped);
                self.center.detach_downlink(&self.id);
                return Ok(());
            }
            LifecycleState::Stopping => {}
            _ => {
                let _ = self.cas_state(cur, LifecycleState::Stopping);
            }
        }
        //注销设备
        self.center.detach_downlink(&self.id);
        let handles: Vec<JoinHandle<()>> = {
            let mut handles = self.handles.lock();
            handles.drain(..).collect()
        };
        for mut handle in handles {
            tokio::select! {
                _ = time::sleep(Duration::from_secs(3)) => {
                    handle.abort();
                }
                _ = &mut handle => {}
            }
        }
        self.store_state(LifecycleState::Stopped);
        Ok(())
    }

    fn state(&self) -> LifecycleState {
        self.load_state()
    }
}

impl Identifiable for Emu {
    fn id(&self) -> &str {
        &self.id
    }
}

impl Executable for Emu {}

async fn wait_for_stop(stop_rx: &mut watch::Receiver<bool>) {
    loop {
        if *stop_rx.borrow() {
            return;
        }
        if stop_rx.changed().await.is_err() {
            return;
        }
    }
}

async fn run_downlink(
    mut rx: tokio::sync::mpsc::Receiver<Vec<DownDataPoint>>,
    commands: Arc<AsyncMutex<Vec<Box<dyn Command>>>>,
    strategies: Arc<AsyncMutex<Vec<Box<dyn Strategy>>>>,
    mut stop_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = wait_for_stop(&mut stop_rx) => break,
            msg = rx.recv() => {
                if let Some(entries) = msg {
                    down(&entries, commands.clone(), strategies.clone()).await;
                }
            }
        }
    }
}

async fn down(
    points: &[DownDataPoint],
    commands: Arc<AsyncMutex<Vec<Box<dyn Command>>>>,
    strategies: Arc<AsyncMutex<Vec<Box<dyn Strategy>>>>,
) {
    {
        let commands = commands.lock().await;
        for cmd in commands.iter() {
            if let Err(e) = cmd.down(points).await {
                tracing::error!("[{}] 处理下行点位出错: {}", cmd.name(), e);
            }
        }
    }

    {
        let strategies = strategies.lock().await;
        for strategy in strategies.iter() {
            if let Err(e) = strategy.down(points).await {
                tracing::error!("[{}] 处理下行点位出错: {}", strategy.name(), e);
            }
        }
    }
}

async fn run_strategy(
    strategies: Arc<AsyncMutex<Vec<Box<dyn Strategy>>>>,
    idx: usize,
    mut stop_rx: watch::Receiver<bool>,
) {
    let (name, schedule) = {
        let strategies = strategies.lock().await;
        let strategy = &strategies[idx];
        (strategy.name().to_owned(), strategy.schedule())
    };
    tracing::info!("[{}策略] 启动", name);
    {
        let mut strategies = strategies.lock().await;
        if let Err(e) = strategies[idx].on_start().await {
            tracing::error!("[{}策略] 启动失败: {}", name, e);
            return;
        }
    }
    match schedule {
        Schedule::Interval(dur) => {
            let mut ticker = tokio::time::interval(dur);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            ticker.tick().await;
            loop {
                tokio::select! {
                    _ = wait_for_stop(&mut stop_rx) => break,
                    _ = ticker.tick() => {
                        let mut strategies = strategies.lock().await;
                        if let Err(e) = strategies[idx].on_tick().await {
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
                    _ = wait_for_stop(&mut stop_rx) => break,
                    _ = tokio::time::sleep(delay) => {
                        let mut strategies = strategies.lock().await;
                        if let Err(e) = strategies[idx].on_tick().await {
                            tracing::error!("[{}策略] 执行出错: {}", name, e);
                        }
                    }
                }
            }
        }
    }
    tracing::info!("[策略] {} 已停止", name);
}
