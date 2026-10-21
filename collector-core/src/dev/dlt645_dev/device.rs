use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{info, warn};

use crate::config::{self, Device, dlt645_conf::Dlt645Configs};
use crate::dev::{
    DeviceError, Executable, Identifiable, Lifecycle, LifecycleState,
    dev_config::{Dlt645ConfError, Dlt645DeviceConfig},
    state::SharedState,
};

use super::runner::{Dlt645Runner, PollItem, build_plan};

/// DL/T 645-2007 主站设备：独占一条 RS-485 串口，按点表轮询总线上的电表并入库。
///
/// 目前只支持读取，没有下行通道（不向数据中心注册 downlink）。
pub struct Dlt645Dev {
    id: String,
    config: Dlt645DeviceConfig,
    plan: Vec<PollItem>,
    state: SharedState,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Dlt645Dev {
    pub fn new(dev: Device) -> Result<Self, DeviceError> {
        let Some(id) = dev.id else {
            return Err(DeviceError::InvalidId);
        };
        let Some(configs) = dev.protocol_configs else {
            return Err(DeviceError::NotFoundConfigs(id));
        };
        let configs: Dlt645Configs = match configs {
            config::ProtocolConfigs::Dlt645(configs) => configs,
            config::ProtocolConfigs::None => return Err(DeviceError::NotFoundConfigs(id)),
            _ => return Err(DeviceError::UnSupportedComType),
        };
        let config = Dlt645DeviceConfig::try_from(dev.config)?;
        let plan = build_plan(configs, config.meter_address);
        if plan.is_empty() {
            return Err(Dlt645ConfError::NoPoints.into());
        }
        let state = SharedState::new(LifecycleState::New);
        let (stop_tx, stop_rx) = watch::channel(false);
        info!("加载{}配置成功!", id);
        Ok(Self {
            id,
            config,
            plan,
            state,
            stop_tx,
            stop_rx,
            task: Mutex::new(None),
        })
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

impl Identifiable for Dlt645Dev {
    fn id(&self) -> &str {
        &self.id
    }
}

#[async_trait::async_trait]
impl Lifecycle for Dlt645Dev {
    fn init(&self) -> Result<(), DeviceError> {
        if !self.cas_state(LifecycleState::New, LifecycleState::Initializing) {
            return Ok(());
        }
        self.store_state(LifecycleState::Ready);
        Ok(())
    }

    async fn start(&mut self) -> Result<(), DeviceError> {
        let ok = self.cas_state(LifecycleState::Ready, LifecycleState::Starting)
            || self.cas_state(LifecycleState::Stopped, LifecycleState::Starting);
        if !ok {
            return Ok(());
        }
        let _ = self.stop_tx.send(false);
        let mut task_guard = self.task.lock().await;
        if let Some(handle) = task_guard.take() {
            handle.abort();
        }
        let runner = Dlt645Runner {
            id: self.id.clone(),
            config: self.config.clone(),
            plan: self.plan.clone(),
            state: self.state.clone(),
            stop_rx: self.stop_rx.clone(),
        };
        let handle = tokio::spawn(async move {
            runner.run().await;
        });
        *task_guard = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<(), DeviceError> {
        let _ = self.stop_tx.send(true);
        let cur = self.load_state();
        match cur {
            LifecycleState::Stopped => return Ok(()),
            LifecycleState::New | LifecycleState::Ready => {
                self.store_state(LifecycleState::Stopped);
                return Ok(());
            }
            LifecycleState::Stopping => {}
            _ => {
                let _ = self.cas_state(cur, LifecycleState::Stopping);
            }
        }
        let mut task_guard = self.task.lock().await;
        if let Some(mut handle) = task_guard.take() {
            tokio::select! {
                _ = time::sleep(Duration::from_secs(3)) => {
                    warn!("[{}] 停止超时, 强制终止任务", self.id);
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

impl Executable for Dlt645Dev {}
