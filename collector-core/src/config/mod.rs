use serde::Deserialize;
use std::collections::HashMap;
use tokio::fs;
use tracing::error;

use crate::config::modbus_conf::{ModbusConfigs, build_configs};

pub mod modbus_conf;

#[derive(Debug, thiserror::Error)]
pub enum ConfigurationError {
    #[error("Failed to read file: {0}")]
    ReadFileError(#[from] std::io::Error),
    #[error("Failed to parse config: {0}")]
    ParseJsonError(#[from] serde_json::Error),
}

#[derive(Debug)]
pub struct Configuration {
    pub project: Project,
}

impl Configuration {
    pub async fn new(path: String) -> Result<Self, ConfigurationError> {
        let mut bytes = fs::read(path.as_str()).await?;
        // strip UTF-8 BOM (EF BB BF)
        if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
            bytes.drain(..3);
        }
        while matches!(bytes.first(), Some(b' ' | b'\n' | b'\r' | b'\t')) {
            bytes.drain(..1);
        }
        let project = serde_json::from_slice::<Project>(&bytes)?;
        Ok(Self { project })
    }

    pub async fn load_device_configs(&mut self) {
        for (_, dev) in self.project.devices.iter_mut() {
            let Some(com) = dev.config.com_type else {
                dev.protocol_configs = Some(ProtocolConfigs::None);
                continue;
            };
            let Some(file) = dev.config.register_file.as_deref() else {
                dev.protocol_configs = Some(ProtocolConfigs::None);
                continue;
            };
            match com {
                ComType::ModbusTCP => {
                    let file = file.to_string();
                    if let Ok(result) = tokio::task::spawn_blocking(|| build_configs(file)).await {
                        match result {
                            Ok(configs) => {
                                dev.protocol_configs = Some(ProtocolConfigs::Modbus(configs))
                            }
                            Err(err) => {
                                error!("Failed to build {:?} configs: {}", dev.id, err);
                                dev.protocol_configs = Some(ProtocolConfigs::None);
                            }
                        }
                    }
                }
                ComType::ModbusRTU => unimplemented!(),
                ComType::CAN => unimplemented!(),
                ComType::IEC104 => unimplemented!(),
                ComType::IEC61850 => unimplemented!(),
            }
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub product_type: Option<String>,
    pub project: Option<String>,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub devices: HashMap<String, Device>,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: Option<String>,
    pub desc: Option<String>,
    pub config: DeviceConfig,

    #[serde(skip)]
    pub protocol_configs: Option<ProtocolConfigs>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
pub enum ComType {
    #[serde(rename = "ModbusTCP")]
    ModbusTCP,
    #[serde(rename = "ModbusRTU")]
    ModbusRTU,
    #[serde(rename = "CAN")]
    CAN,
    #[serde(rename = "IEC104")]
    IEC104,
    #[serde(rename = "IEC61850")]
    IEC61850,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceConfig {
    #[serde(rename = "type")]
    pub device_type: Option<String>,
    #[serde(rename = "comType")]
    pub com_type: Option<ComType>,
    pub register_file: Option<String>,
    pub interval: Option<u64>,
    pub timeout: Option<u64>,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub slave: Option<u8>,
    pub serial_tty: Option<String>,
    pub baud_rate: Option<u32>,
    pub data_bits: Option<u8>,
    pub parity: Option<String>,
    pub stop_bits: Option<u8>,
    pub interface: Option<String>,
    pub desc: Option<String>,
}

#[derive(Debug, Clone)]
pub enum ProtocolConfigs {
    Modbus(ModbusConfigs),
    None,
}
