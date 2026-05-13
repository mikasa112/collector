use std::{collections::HashMap, sync::Arc};

use tokio::sync::Mutex;
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tracing::error;

use crate::center::SharedPointCenter;
use crate::config::{ComType, Device};

#[cfg(target_os = "linux")]
use crate::dev::can_dev::CanDev;
#[cfg(target_os = "linux")]
use crate::dev::gpio::GpioDev;
use crate::{
    config,
    dev::{DeviceError, Executable, modbus_dev::ModbusDev},
};

pub struct DevManager {
    devices: Vec<Arc<Mutex<Box<dyn Executable>>>>,
    tasks: JoinSet<()>,
    cancel_token: Option<CancellationToken>,
}

impl DevManager {
    pub fn new(map: HashMap<String, Device>, center: SharedPointCenter) -> Self {
        let mut devices: Vec<Arc<Mutex<Box<dyn Executable>>>> = Vec::new();
        for (_, dev) in map.into_iter() {
            let Some(com_type) = dev.config.com_type else {
                continue;
            };
            let center_clone = center.clone();
            match init_device(dev, com_type, center_clone) {
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
            cancel_token: None,
        }
    }

    pub fn set_cancel_token(&mut self, token: CancellationToken) {
        self.cancel_token = Some(token);
    }

    pub fn add_device(&mut self, device: Arc<Mutex<Box<dyn Executable>>>) {
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

    pub async fn find_dev(&self, id: &str) -> Option<Arc<Mutex<Box<dyn Executable>>>> {
        for dev in self.devices.iter() {
            let dev_mutex = dev.lock().await;
            if dev_mutex.id() == id {
                return Some(dev.clone());
            }
        }
        None
    }
}

fn init_device(
    dev: Device,
    com_type: ComType,
    center: SharedPointCenter,
) -> Result<Arc<Mutex<Box<dyn Executable>>>, DeviceError> {
    let my_dev: Box<dyn Executable> = match com_type {
        config::ComType::ModbusTCP | config::ComType::ModbusRTU => {
            Box::new(ModbusDev::new(dev, center)?)
        }
        #[cfg(target_os = "linux")]
        config::ComType::CAN => Box::new(CanDev::new(dev, center)?),
        #[cfg(not(target_os = "linux"))]
        config::ComType::CAN => return Err(DeviceError::UnSupportedComType),
        config::ComType::IEC104 => todo!(),
        config::ComType::IEC61850 => todo!(),
        #[cfg(target_os = "linux")]
        config::ComType::GPIO => Box::new(GpioDev::new(dev, center)?),
        #[cfg(not(target_os = "linux"))]
        config::ComType::GPIO => return Err(DeviceError::UnSupportedComType),
    };
    my_dev.init()?;
    Ok(Arc::new(Mutex::new(my_dev)))
}
