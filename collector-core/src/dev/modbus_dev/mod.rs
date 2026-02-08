mod backoff;
mod batch;
mod device;
mod error;
mod runner;
mod state;

pub use device::ModbusDev;
pub use error::ModbusDevError;

use crate::dev::dev_config::{ModbusRtuConfig, ModbusTcpConfig};

#[derive(Clone)]
pub(super) enum Protocol {
    TCP(ModbusTcpConfig),
    RTU(ModbusRtuConfig),
}
