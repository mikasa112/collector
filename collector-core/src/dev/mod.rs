pub mod manager;
pub mod modbus_dev;

pub enum DeviceError {}

pub trait Identifiable: Sync + Send {
    fn id(&self) -> String;
}

pub enum LifecycleState {
    Starting,
    Connecting,
    Running,
    Stopping,
    Stopped,
}

#[async_trait::async_trait]
pub trait Lifecycle {
    async fn start(&self) -> Result<(), DeviceError>;
    async fn stop(&self) -> Result<(), DeviceError>;
    async fn connect(&self) -> Result<(), DeviceError>;
    async fn disconnect(&self) -> Result<(), DeviceError>;
    async fn reconnect(&self) -> Result<(), DeviceError>;
    fn state(&self) -> LifecycleState;
}

pub trait Executable: Identifiable + Lifecycle {}
