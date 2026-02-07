use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tracing::error;

use crate::config::{ComType, Device};

use crate::dev::Lifecycle;
use crate::{
    config,
    dev::{DeviceError, Executable, modbus_dev::ModbusDev},
};

pub struct DevManager {
    devices: Vec<Arc<Mutex<dyn Executable>>>,
    tasks: JoinSet<()>,
}

impl DevManager {
    pub fn new(map: HashMap<String, Device>) -> Self {
        let mut devices: Vec<Arc<Mutex<dyn Executable>>> = Vec::new();
        for (_, dev) in map.into_iter() {
            let Some(com_type) = dev.config.com_type else {
                continue;
            };
            match init_device(dev, com_type) {
                Ok(dev) => {
                    devices.push(dev);
                }
                Err(err) => {
                    error!("{}", err)
                }
            }
        }
        DevManager {
            devices,
            tasks: JoinSet::new(),
        }
    }

    pub fn add_device(&mut self, device: Arc<Mutex<dyn Executable>>) {
        self.devices.push(device);
    }

    pub async fn start_all(&mut self) {
        for dev in self.devices.iter() {
            let dev_clone = Arc::clone(dev);
            self.tasks.spawn(async move {
                let mut dev_clone_mutex = dev_clone.lock().await;
                if let Err(err) = dev_clone_mutex.start().await {
                    error!("{}", err);
                }
            });
        }
    }

    pub async fn stop_all(&mut self) {
        for dev in self.devices.iter() {
            let dev_mutex = dev.lock().await;
            if let Err(err) = dev_mutex.stop().await {
                error!("{}", err);
            }
        }
        while let Some(res) = self.tasks.join_next().await {
            if let Err(err) = res {
                error!("{}", err);
            }
        }
    }
}

fn init_device(dev: Device, com_type: ComType) -> Result<Arc<Mutex<dyn Executable>>, DeviceError> {
    let my_dev = match com_type {
        config::ComType::ModbusTCP => ModbusDev::new(dev)?,
        config::ComType::ModbusRTU => ModbusDev::new(dev)?,
        config::ComType::CAN => todo!(),
        config::ComType::IEC104 => todo!(),
        config::ComType::IEC61850 => todo!(),
    };
    my_dev.init()?;
    Ok(Arc::new(Mutex::new(my_dev)))
}
