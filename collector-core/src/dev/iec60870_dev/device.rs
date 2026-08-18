use collector_iec60870::Client;

use crate::dev::{DeviceError, Executable, Identifiable, Lifecycle, LifecycleState};

pub struct Iec60870Dev {
    id: String,
}

impl Iec60870Dev {
    #[allow(dead_code)]
    pub fn new() -> Result<Self, DeviceError> {
        unimplemented!("尚未实现")
    }
}

impl Identifiable for Iec60870Dev {
    fn id(&self) -> &str {
        &self.id
    }
}

#[async_trait::async_trait]
impl Lifecycle for Iec60870Dev {
    fn init(&self) -> Result<(), DeviceError> {
        let _conn = Client::new("127.0.0.1", 2404);
        todo!()
    }

    async fn start(&mut self) -> Result<(), DeviceError> {
        todo!()
    }

    async fn stop(&self) -> Result<(), DeviceError> {
        todo!()
    }
    fn state(&self) -> LifecycleState {
        todo!()
    }
}

impl Executable for Iec60870Dev {}
