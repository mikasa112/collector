use std::fmt;

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

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleState {
    New = 0,
    Initializing = 1,
    Ready = 2,
    Starting = 3,
    Connecting = 4,
    Connected = 5,
    Running = 6,
    Stopping = 7,
    Stopped = 8,
    Failed = 9,
}

impl From<u8> for LifecycleState {
    fn from(value: u8) -> Self {
        match value {
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
}

impl fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LifecycleState::New => write!(f, "新建"),
            LifecycleState::Initializing => write!(f, "初始化中"),
            LifecycleState::Ready => write!(f, "准备就绪"),
            LifecycleState::Starting => write!(f, "启动中"),
            LifecycleState::Connecting => write!(f, "连接中"),
            LifecycleState::Connected => write!(f, "连接成功"),
            LifecycleState::Running => write!(f, "运行中"),
            LifecycleState::Stopping => write!(f, "停止中"),
            LifecycleState::Stopped => write!(f, "停止成功"),
            LifecycleState::Failed => write!(f, "失败"),
        }
    }
}

#[async_trait::async_trait]
pub trait Lifecycle {
    fn init(&self) -> Result<(), DeviceError>;
    async fn start(&mut self) -> Result<(), DeviceError>;
    async fn stop(&self) -> Result<(), DeviceError>;
    fn state(&self) -> LifecycleState;
}

pub trait Executable: Identifiable + Lifecycle {}
