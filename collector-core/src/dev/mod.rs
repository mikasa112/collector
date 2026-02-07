use crate::{
    center::DataCenterError,
    dev::dev_config::{ModbusRtuConfError, ModbusTcpConfError},
};

pub mod can_dev;
pub(crate) mod dev_config;
pub mod manager;
pub mod modbus_dev;

#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    #[error("无效的ID")]
    InvalidId,
    #[error("无效的通信类型")]
    InvalidComType,
    #[error("不支持的通信类型")]
    UnSupportedComType,
    #[error("Modbus TCP配置错误")]
    ModbusTcpConfigError(#[from] ModbusTcpConfError),
    #[error("Modbus RTU配置错误")]
    ModbusRtuConfigError(#[from] ModbusRtuConfError),
    #[error("{0}找不到点位表")]
    NotFoundConfigs(String),
    #[error("数据中心错误")]
    DCenterError(#[from] DataCenterError),
}

pub trait Identifiable: Sync + Send {
    fn id(&self) -> String;
}

pub enum LifecycleState {
    Initializing,
    Starting,
    Connecting,
    Running,
    Stopping,
    Stopped,
}

#[async_trait::async_trait]
pub trait Lifecycle {
    fn init(&self) -> Result<(), DeviceError>;
    async fn start(&self) -> Result<(), DeviceError>;
    async fn stop(&self) -> Result<(), DeviceError>;
    fn state(&self) -> LifecycleState;
}

pub trait Executable: Identifiable + Lifecycle {}
