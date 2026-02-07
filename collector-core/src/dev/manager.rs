use std::{collections::HashMap, sync::Arc};

use tokio::task::JoinSet;
use tracing::error;

use crate::config::{ComType, Device};

use crate::{
    config,
    dev::{DeviceError, Executable, modbus_dev::ModbusDev},
};

pub struct DevManager {
    devices: Vec<Arc<dyn Executable>>,
    tasks: JoinSet<()>,
}

impl DevManager {
    pub fn new(map: HashMap<String, Device>) -> Self {
        let mut devices: Vec<Arc<dyn Executable>> = Vec::new();
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

    pub fn add_device(&mut self, device: Arc<dyn Executable>) {
        self.devices.push(device);
    }

    pub async fn start_all(&mut self) {
        for dev in self.devices.iter() {
            let dev_clone = Arc::clone(dev);
            self.tasks.spawn(async move {
                if let Err(err) = dev_clone.start().await {
                    error!("{}", err);
                }
            });
        }
    }

    pub async fn stop_all(&mut self) {
        for dev in self.devices.iter() {
            if let Err(err) = dev.stop().await {
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

fn init_device(dev: Device, com_type: ComType) -> Result<Arc<dyn Executable>, DeviceError> {
    match com_type {
        config::ComType::ModbusTCP => Ok(Arc::new(ModbusDev::new(dev)?)),
        config::ComType::ModbusRTU => Ok(Arc::new(ModbusDev::new(dev)?)),
        config::ComType::CAN => todo!(),
        config::ComType::IEC104 => todo!(),
        config::ComType::IEC61850 => todo!(),
    }
}
