use calamine::{Data, DataType};
use serde::Deserialize;
use std::collections::HashMap;
use tokio::fs;
use tracing::error;

pub mod can_conf;
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
            dev.protocol_configs = Some(load_protocol_configs(dev).await);
        }
    }
}

async fn load_protocol_configs(dev: &Device) -> ProtocolConfigs {
    let Some(com) = dev.config.com_type else {
        return ProtocolConfigs::None;
    };
    let Some(file) = dev.config.register_file.clone() else {
        return ProtocolConfigs::None;
    };
    let dev_id = dev.id.clone();

    match com {
        ComType::ModbusTCP | ComType::ModbusRTU => {
            load_configs(file, dev_id, modbus_conf::build_configs, ProtocolConfigs::Modbus).await
        }
        #[cfg(target_os = "linux")]
        ComType::CAN => {
            load_configs(file, dev_id, can_conf::build_configs, ProtocolConfigs::CAN).await
        }
        #[cfg(not(target_os = "linux"))]
        ComType::CAN => {
            error!("Failed to build {:?} configs: CAN is only supported on Linux", dev_id);
            ProtocolConfigs::None
        }
        ComType::IEC104 => unimplemented!(),
        ComType::IEC61850 => unimplemented!(),
    }
}

async fn load_configs<T, E, B, W>(
    file: String,
    dev_id: Option<String>,
    build: B,
    wrap: W,
) -> ProtocolConfigs
where
    T: Send + 'static,
    E: std::fmt::Display + Send + 'static,
    B: FnOnce(String) -> Result<T, E> + Send + 'static,
    W: FnOnce(T) -> ProtocolConfigs,
{
    match tokio::task::spawn_blocking(move || build(file)).await {
        Ok(Ok(configs)) => wrap(configs),
        Ok(Err(err)) => {
            error!("Failed to build {:?} configs: {}", dev_id, err);
            ProtocolConfigs::None
        }
        Err(err) => {
            error!("Failed to join config loader for {:?}: {}", dev_id, err);
            ProtocolConfigs::None
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
    Modbus(modbus_conf::ModbusConfigs),
    #[cfg(target_os = "linux")]
    CAN(can_conf::CanConfigs),
    None,
}

pub(crate) fn required_f64(row: &[Data], idx: usize, field: &str) -> Result<f64, anyhow::Error> {
    row[idx]
        .get_float()
        .ok_or_else(|| anyhow::Error::msg(format!("{field}不能为空")))
}

pub(crate) fn required_str<'a>(
    row: &'a [Data],
    idx: usize,
    field: &str,
) -> Result<&'a str, anyhow::Error> {
    row[idx]
        .get_string()
        .ok_or_else(|| anyhow::Error::msg(format!("{field}不能为空")))
}

pub(crate) fn required_static_str(
    row: &[Data],
    idx: usize,
    field: &str,
) -> Result<&'static str, anyhow::Error> {
    Ok(required_str(row, idx, field)?.to_owned().leak())
}

pub(crate) fn optional_static_str(row: &[Data], idx: usize) -> Option<&'static str> {
    row[idx].get_string().map(|s| {
        let leaked: &'static mut str = s.to_owned().leak();
        leaked as &'static str
    })
}

/// 获取u16近似整数
pub(crate) fn required_usize_integerish(
    row: &[Data],
    idx: usize,
    field: &str,
) -> Result<usize, anyhow::Error> {
    if let Some(v) = row[idx].get_int() {
        return usize::try_from(v).map_err(|_| anyhow::Error::msg(format!("{field}超出范围")));
    }
    if let Some(v) = row[idx].get_float() {
        if !v.is_finite() || v.fract().abs() > f64::EPSILON {
            return Err(anyhow::Error::msg(format!("{field}必须是整数")));
        }
        return Ok(v as usize);
    }
    Err(anyhow::Error::msg(format!("{field}不能为空")))
}

pub(crate) fn required_hex(row: &[Data], idx: usize, field: &str) -> Result<u32, anyhow::Error> {
    let str = row[idx]
        .get_string()
        .ok_or_else(|| anyhow::Error::msg(format!("{field}不能为空")))?
        .trim();
    let str = str.strip_prefix("0x").unwrap_or(str);
    u32::from_str_radix(str, 16).map_err(|_| anyhow::Error::msg(format!("{field}格式错误")))
}
