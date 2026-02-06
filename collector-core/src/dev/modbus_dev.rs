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
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<Entry>>(16);
        info!("加载{}配置成功!", id);
        Ok(ModbusDev {
            id,
            protocol,
            configs,
            tx,
            rx,
        })
    }
}

impl Identifiable for ModbusDev {
    fn id(&self) -> String {
        return self.id.clone();
    }
}

#[async_trait::async_trait]
impl Lifecycle for ModbusDev {
    async fn start(&self) -> Result<(), DeviceError> {
        let tx = self.tx.clone();
        global_center().attach(self, tx);
        unimplemented!()
    }
    async fn stop(&self) -> Result<(), DeviceError> {
        global_center().detach(self);
        unimplemented!()
    }
    fn state(&self) -> LifecycleState {
        unimplemented!()
    }
}

impl Executable for ModbusDev {}
