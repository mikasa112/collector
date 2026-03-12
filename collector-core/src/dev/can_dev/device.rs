use std::sync::{Arc, atomic::AtomicU8};

use crate::{
    config::can_conf::CanConfigs,
    dev::{DeviceError, Lifecycle, LifecycleState},
};

pub struct CanDev {
    id: String,
    configs: CanConfigs,
    state: Arc<AtomicU8>,
}

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
