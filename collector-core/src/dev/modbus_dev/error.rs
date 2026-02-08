use std::net::AddrParseError;

use tokio_modbus::{Error as ModbusError, ExceptionCode};

#[derive(Debug, thiserror::Error)]
pub enum ModbusDevError {
    #[error("IP parse error: {0}")]
    IpParseError(#[from] AddrParseError),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Timeout")]
    Elapsed(#[from] tokio::time::error::Elapsed),
    #[error("Serial port error: {0}")]
    SerialError(#[from] tokio_serial::Error),
    #[error("Modbus error: {0}")]
    ModbusError(#[from] ModbusError),
    #[error("Modbus exception: {0:?}")]
    ModbusException(ExceptionCode),
}

impl From<ExceptionCode> for ModbusDevError {
    fn from(value: ExceptionCode) -> Self {
        ModbusDevError::ModbusException(value)
    }
}
