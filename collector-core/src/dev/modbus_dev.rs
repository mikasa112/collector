use collector_cmd::config::Device;
use collector_cmd::config::modbus_conf::ModbusConfigs;
use tokio_modbus::{
    Slave,
    client::{rtu, tcp},
};
use tokio_serial::SerialStream;

use crate::dev::{
    DeviceError, Identifiable, Lifecycle, LifecycleState,
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
            return Err(DeviceError::NotFoundConfigs);
        };
        let configs = match configs {
            collector_cmd::config::ProtocolConfigs::Modbus(modbus_configs) => modbus_configs,
            collector_cmd::config::ProtocolConfigs::None => {
                return Err(DeviceError::NotFoundConfigs);
            }
        };
        let protocol = match com_type {
            collector_cmd::config::ComType::ModbusTCP => {
                let tcp_config = ModbusTcpConfig::try_from(dev.config)?;
                Ok(Protocol::TCP(tcp_config))
            }
            collector_cmd::config::ComType::ModbusRTU => {
                let rtu_config = ModbusRtuConfig::try_from(dev.config)?;
                Ok(Protocol::RTU(rtu_config))
            }
            _ => Err(DeviceError::UnSupportedComType),
        }?;
        Ok(ModbusDev {
            id,
            protocol,
            configs,
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
        unimplemented!()
    }
    async fn stop(&self) -> Result<(), DeviceError> {
        unimplemented!()
    }
    fn state(&self) -> LifecycleState {
        unimplemented!()
    }
}

#[async_trait::async_trait]
trait ModbusTransport {
    async fn connect(&mut self) -> Result<(), DeviceError>;
    async fn disconnect(&mut self);
}

fn rtu(config: ModbusRtuConfig) {
    let builder = tokio_serial::new(config.serial_tty, config.baudrate);
    let port = SerialStream::open(&builder).unwrap();
    let mut ctx = rtu::attach_slave(port, Slave(config.slave));
}

async fn tcp(config: ModbusTcpConfig) {
    let addr = format!("{}:{}", config.ip, config.port).parse().unwrap();
    let mut ctx = tcp::connect_slave(addr, Slave(config.slave)).await.unwrap();
}
