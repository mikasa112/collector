mod backoff;
mod block;
mod device;
mod downlink;
mod error;
mod runner;
mod state;

pub use device::ModbusDev;
pub use error::ModbusDevError;

use crate::dev::dev_config::{ModbusRtuConfig, ModbusTcpConfig};

#[derive(Clone)]
pub(super) enum Protocol {
    Tcp(ModbusTcpConfig),
    Rtu(ModbusRtuConfig),
}
