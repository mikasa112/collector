use crate::dev::Executable;

pub struct DevManager {
    devices: Vec<Box<dyn Executable>>,
}

impl DevManager {
    pub fn new() -> Self {
        DevManager {
            devices: Vec::new(),
        }
    }

    pub fn add_device(&mut self, device: Box<dyn Executable>) {
        self.devices.push(device);
    }

    pub async fn start_all(&self) {
        for dev in self.devices.iter() {
            dev.start().await;
        }
    }
}
