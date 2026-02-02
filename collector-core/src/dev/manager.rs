use std::sync::Arc;

use crate::dev::Executable;

pub struct DevManager {
    devices: Vec<Arc<dyn Executable>>,
}

impl DevManager {
    pub fn new() -> Self {
        DevManager {
            devices: Vec::new(),
        }
    }

    pub fn add_device(&mut self, device: Arc<dyn Executable>) {
        self.devices.push(device);
    }

    pub async fn start_all(&self) {
        for dev in self.devices.iter() {
            dev.start().await;
        }
    }

    pub async fn stop_all(&self) {
        for dev in self.devices.iter() {
            dev.stop().await;
        }
    }
}
