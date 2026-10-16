use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{info, warn};

use crate::center::{DataCenterError, data_center};
use crate::config::{self, Device, iec104_conf::Iec104Configs};
use crate::core::point::DownDataPoint;
use crate::dev::{
    DeviceError, Executable, Identifiable, Lifecycle, LifecycleState,
    dev_config::Iec104DeviceConfig, state::SharedState,
};

use super::runner::Iec60870Runner;

/// IEC 60870-5-104 主站设备：作为客户端连接对端子站，总召/接收遥信遥测，并下发遥控遥调。
pub struct Iec60870Dev {
    id: String,
    config: Iec104DeviceConfig,
    configs: Iec104Configs,
    state: SharedState,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl Iec60870Dev {
    pub fn new(dev: Device) -> Result<Self, DeviceError> {
        let Some(id) = dev.id else {
            return Err(DeviceError::InvalidId);
        };
        let Some(configs) = dev.protocol_configs else {
            return Err(DeviceError::NotFoundConfigs(id));
        };
        let configs: Iec104Configs = match configs {
            config::ProtocolConfigs::Iec104(configs) => configs,
            config::ProtocolConfigs::None => return Err(DeviceError::NotFoundConfigs(id)),
            _ => return Err(DeviceError::UnSupportedComType),
        }
        .into_iter()
        .filter(|cfg| cfg.enable)
        .collect();
        let config = Iec104DeviceConfig::try_from(dev.config)?;
        let state = SharedState::new(LifecycleState::New);
        let (stop_tx, stop_rx) = watch::channel(false);
        info!("加载{}配置成功!", id);
        Ok(Self {
            id,
            config,
            configs,
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

impl Identifiable for Iec60870Dev {
    fn id(&self) -> &str {
        &self.id
    }
}

#[async_trait::async_trait]
impl Lifecycle for Iec60870Dev {
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
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<DownDataPoint>>(16);
        match data_center().attach_downlink(&self.id, tx.clone()) {
            Ok(()) => {}
            Err(DataCenterError::DevHasRegister(_)) => {
                data_center().detach_downlink(&self.id);
                if let Err(err) = data_center().attach_downlink(&self.id, tx) {
                    warn!("[{}] 重新注册数据中心失败: {}", self.id, err);
                    return Ok(());
                }
            }
            Err(err) => {
                warn!("[{}] 重新注册数据中心失败: {}", self.id, err);
                return Ok(());
            }
        }
        let _ = self.stop_tx.send(false);
        let mut task_guard = self.task.lock().await;
        if let Some(handle) = task_guard.take() {
            handle.abort();
        }
        let runner = Iec60870Runner {
            id: self.id.clone(),
            config: self.config.clone(),
            configs: self.configs.clone(),
            state: self.state.clone(),
            stop_rx: self.stop_rx.clone(),
            rx,
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
                data_center().detach_downlink(&self.id);
                return Ok(());
            }
            LifecycleState::Stopping => {}
            _ => {
                let _ = self.cas_state(cur, LifecycleState::Stopping);
            }
        }
        data_center().detach_downlink(&self.id);
        let mut task_guard = self.task.lock().await;
        if let Some(mut handle) = task_guard.take() {
            tokio::select! {
                _ = time::sleep(Duration::from_secs(3)) => {
                    handle.abort();
                }
                _ = &mut handle => {}
            }
        }
        Ok(())
    }

    fn state(&self) -> LifecycleState {
        self.load_state()
    }
}

impl Executable for Iec60870Dev {}
