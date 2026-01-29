use crate::dev::{DeviceError, Identifiable, Lifecycle, LifecycleState};

pub struct ModbusDev {}

impl Identifiable for ModbusDev {
    fn id(&self) -> String {
        unimplemented!()
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
    async fn connect(&self) -> Result<(), DeviceError> {
        unimplemented!()
    }
    async fn disconnect(&self) -> Result<(), DeviceError> {
        unimplemented!()
    }
    async fn reconnect(&self) -> Result<(), DeviceError> {
        unimplemented!()
    }

    fn state(&self) -> LifecycleState {
        unimplemented!()
    }
}
