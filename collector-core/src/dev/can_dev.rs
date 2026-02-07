use crate::dev::{DeviceError, Lifecycle, LifecycleState};

pub struct CanDev {}

impl CanDev {}

#[async_trait::async_trait]
impl Lifecycle for CanDev {
    fn init(&self) -> Result<(), DeviceError> {
        unimplemented!()
    }
    async fn start(&mut self) -> Result<(), DeviceError> {
        unimplemented!()
    }
    async fn stop(&self) -> Result<(), DeviceError> {
        unimplemented!()
    }
    fn state(&self) -> LifecycleState {
        unimplemented!()
    }
}
