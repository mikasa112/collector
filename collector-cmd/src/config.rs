use std::collections::HashMap;

use serde::Deserialize;
use tokio::fs;

#[derive(Debug, thiserror::Error)]
pub enum ConfigCenterError {
    #[error("Failed to read file: {0}")]
    ReadFileError(#[from] std::io::Error),
    #[error("Failed to parse config: {0}")]
    ParseJsonError(#[from] serde_json::Error),
}

#[derive(Debug)]
pub struct ConfigCenter {
    pub project: Project,
}

impl ConfigCenter {
    pub async fn new(path: String) -> Result<Self, ConfigCenterError> {
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
    pub parity: Option<String>,
    pub stop_bits: Option<u8>,
    pub interface: Option<String>,
    pub desc: Option<String>,
}
