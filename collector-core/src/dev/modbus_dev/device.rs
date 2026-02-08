use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::time::Duration;

use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time;
use tracing::{info, warn};

use crate::center::{Center, DataCenterError, global_center};
use crate::config::modbus_conf::ModbusConfigs;
use crate::config::{self, Device};
use crate::dev::modbus_dev::Protocol;
use crate::dev::{
    DeviceError, Executable, Identifiable, Lifecycle, LifecycleState,
    dev_config::{ModbusRtuConfig, ModbusTcpConfig},
};

use super::runner::ModbusRunner;
use super::state::{cas_state, load_state, store_state};

pub struct ModbusDev {
    id: String,
    protocol: Protocol,
    configs: ModbusConfigs,
    state: Arc<AtomicU8>,
    tx: mpsc::Sender<Vec<crate::center::data_center::Entry>>,
    _rx: mpsc::Receiver<Vec<crate::center::data_center::Entry>>,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl ModbusDev {
    pub fn new(dev: Device) -> Result<Self, DeviceError> {
        let Some(id) = dev.id else {
            return Err(DeviceError::InvalidId);
        };
        let Some(com_type) = dev.config.com_type else {
            return Err(DeviceError::InvalidComType);
        };
        let Some(configs) = dev.protocol_configs else {
            return Err(DeviceError::NotFoundConfigs(id));
        };
        let configs = match configs {
            config::ProtocolConfigs::Modbus(modbus_configs) => modbus_configs,
            config::ProtocolConfigs::None => {
                return Err(DeviceError::NotFoundConfigs(id));
            }
        };
        let protocol = match com_type {
            config::ComType::ModbusTCP => {
                let tcp_config = ModbusTcpConfig::try_from(dev.config)?;
                Ok(Protocol::Tcp(tcp_config))
            }
            config::ComType::ModbusRTU => {
                let rtu_config = ModbusRtuConfig::try_from(dev.config)?;
                Ok(Protocol::Rtu(rtu_config))
            }
            _ => Err(DeviceError::UnSupportedComType),
        }?;
        let state = Arc::new(AtomicU8::new(LifecycleState::New as u8));
        let (tx, _rx) = tokio::sync::mpsc::channel::<Vec<crate::center::data_center::Entry>>(16);
        let (stop_tx, stop_rx) = watch::channel(false);
        info!("加载{}配置成功!", id);
        Ok(ModbusDev {
            id,
            protocol,
            state,
            configs,
            tx,
            _rx,
            stop_tx,
            stop_rx,
            task: Mutex::new(None),
        })
    }

    fn load_state(&self) -> LifecycleState {
        load_state(&self.state)
    }

    fn cas_state(&self, from: LifecycleState, to: LifecycleState) -> bool {
        cas_state(&self.state, from, to)
    }

    fn store_state(&self, to: LifecycleState) {
        store_state(&self.id, &self.state, to);
    }
}

impl Identifiable for ModbusDev {
    fn id(&self) -> String {
        self.id.clone()
    }
}

#[async_trait::async_trait]
impl Lifecycle for ModbusDev {
    fn init(&self) -> Result<(), DeviceError> {
        if !self.cas_state(LifecycleState::New, LifecycleState::Initializing) {
            return Ok(());
        }
        let tx = self.tx.clone();
        global_center().attach(self, tx)?;
        self.store_state(LifecycleState::Ready);
        Ok(())
    }

    async fn start(&mut self) -> Result<(), DeviceError> {
        let ok = self.cas_state(LifecycleState::Ready, LifecycleState::Starting)
            || self.cas_state(LifecycleState::Stopped, LifecycleState::Starting);
        if !ok {
            return Ok(());
        }
        let tx = self.tx.clone();
        match global_center().attach(self, tx) {
            Ok(()) => {}
            Err(DataCenterError::DevHasRegister(_)) => {}
            Err(err) => {
                warn!("[{}] 重新注册数据中心失败: {}", self.id, err);
            }
        }
        let _ = self.stop_tx.send(false);
        let mut task_guard = self.task.lock().await;
        if let Some(handle) = task_guard.take() {
            handle.abort();
        }
        let runner = ModbusRunner {
            id: self.id.clone(),
            protocol: self.protocol.clone(),
            configs: self.configs.clone(),
            state: Arc::clone(&self.state),
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
                global_center().detach(self);
                return Ok(());
            }
            LifecycleState::Stopping => {}
            _ => {
                let _ = self.cas_state(cur, LifecycleState::Stopping);
            }
        }

        global_center().detach(self);
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

impl Executable for ModbusDev {}
