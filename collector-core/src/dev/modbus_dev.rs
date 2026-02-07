use std::sync::atomic::{AtomicU8, Ordering};

use crate::center::data_center::Entry;
use crate::center::{Center, global_center};
use crate::config::modbus_conf::ModbusConfigs;
use crate::config::{self, Device};
use tokio::sync::mpsc;
use tracing::info;

use crate::dev::{
    DeviceError, Executable, Identifiable, Lifecycle, LifecycleState,
    dev_config::{ModbusRtuConfig, ModbusTcpConfig},
};

pub enum Protocol {
    TCP(ModbusTcpConfig),
    RTU(ModbusRtuConfig),
}

pub struct ModbusDev {
    pub id: String,
    pub protocol: Protocol,
    pub configs: ModbusConfigs,
    pub state: AtomicU8,
    pub tx: mpsc::Sender<Vec<Entry>>,
    pub rx: mpsc::Receiver<Vec<Entry>>,
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
                Ok(Protocol::TCP(tcp_config))
            }
            config::ComType::ModbusRTU => {
                let rtu_config = ModbusRtuConfig::try_from(dev.config)?;
                Ok(Protocol::RTU(rtu_config))
            }
            _ => Err(DeviceError::UnSupportedComType),
        }?;
        let state = AtomicU8::new(LifecycleState::New as u8);
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<Entry>>(16);
        info!("加载{}配置成功!", id);
        Ok(ModbusDev {
            id,
            protocol,
            state,
            configs,
            tx,
            rx,
        })
    }

    fn load_state(&self) -> LifecycleState {
        match self.state.load(Ordering::Acquire) {
            0 => LifecycleState::New,
            1 => LifecycleState::Initializing,
            2 => LifecycleState::Ready,
            3 => LifecycleState::Starting,
            4 => LifecycleState::Connecting,
            5 => LifecycleState::Connected,
            6 => LifecycleState::Running,
            7 => LifecycleState::Stopping,
            8 => LifecycleState::Stopped,
            9 => LifecycleState::Failed,
            _ => LifecycleState::Failed,
        }
    }

    fn cas_state(&self, from: LifecycleState, to: LifecycleState) -> bool {
        self.state
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn store_state(&self, to: LifecycleState) {
        let from = self.load_state();
        self.state.store(to as u8, Ordering::Release);
        info!("[{}]{} -> {}", self.id, from, to);
    }
}

impl Identifiable for ModbusDev {
    fn id(&self) -> String {
        return self.id.clone();
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
        Ok(())
    }

    async fn stop(&self) -> Result<(), DeviceError> {
        if !self.cas_state(LifecycleState::Running, LifecycleState::Stopping) {
            return Ok(());
        }
        global_center().detach(self);
        self.store_state(LifecycleState::Stopped);
        Ok(())
    }

    fn state(&self) -> LifecycleState {
        self.load_state()
    }
}

impl Executable for ModbusDev {}
