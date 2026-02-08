use std::net::AddrParseError;

use tokio_modbus::{Error as ModbusError, ExceptionCode};

use crate::dev::modbus_dev::block::BuildBlocksError;

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
    #[error("Build blocks error: {0}")]
    BlocksError(#[from] BuildBlocksError),
}

impl From<ExceptionCode> for ModbusDevError {
    fn from(value: ExceptionCode) -> Self {
        ModbusDevError::ModbusException(value)
    }
}
