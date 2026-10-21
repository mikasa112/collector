use calamine::{Data, DataType};
use serde::Deserialize;
use std::collections::HashMap;
use tokio::fs;
use tracing::error;

use crate::core::point::PointId;

pub mod can_conf;
pub mod config_provider;
pub mod dlt645_conf;
pub mod gpio_conf;
pub mod iec104_conf;
pub mod iec61850_conf;
pub mod modbus_conf;
pub mod north_iec104_conf;
pub mod north_modbus_conf;
pub mod program_conf;

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
        for dev in self.project.devices.values_mut() {
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
            load_configs(
                file,
                dev_id,
                modbus_conf::build_configs,
                ProtocolConfigs::Modbus,
            )
            .await
        }
        #[cfg(target_os = "linux")]
        ComType::CAN => {
            load_configs(file, dev_id, can_conf::build_configs, ProtocolConfigs::CAN).await
        }
        #[cfg(not(target_os = "linux"))]
        ComType::CAN => {
            error!(
                "Failed to build {:?} configs: CAN is only supported on Linux",
                dev_id
            );
            ProtocolConfigs::None
        }
        ComType::IEC104 => {
            load_configs(
                file,
                dev_id,
                iec104_conf::build_configs,
                ProtocolConfigs::Iec104,
            )
            .await
        }
        ComType::IEC61850 => {
            load_configs(
                file,
                dev_id,
                iec61850_conf::build_configs,
                ProtocolConfigs::Iec61850,
            )
            .await
        }
        ComType::DLT645 => {
            load_configs(
                file,
                dev_id,
                dlt645_conf::build_configs,
                ProtocolConfigs::Dlt645,
            )
            .await
        }
        #[cfg(target_os = "linux")]
        ComType::GPIO => {
            load_configs(
                file,
                dev_id,
                gpio_conf::build_configs,
                ProtocolConfigs::GPIO,
            )
            .await
        }
        #[cfg(not(target_os = "linux"))]
        ComType::GPIO => {
            error!(
                "Failed to build {:?} configs: GPIO is only supported on Linux",
                dev_id
            );
            ProtocolConfigs::None
        }
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
pub struct Project {
    pub product_type: Option<String>,
    pub project: Option<String>,
    #[serde(default)]
    pub program: program_conf::Program,
    pub devices: HashMap<String, Device>,
    pub mqtt_routes: Option<Vec<MqttRoute>>,
}

#[derive(Deserialize, Clone, Debug)]
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
    #[serde(rename = "GPIO")]
    GPIO,
    #[serde(rename = "DLT645")]
    DLT645,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceConfig {
    #[serde(rename = "type")]
    pub device_type: Option<String>,
    pub com_type: Option<ComType>,
    pub register_file: Option<String>,
    pub interval: Option<u64>,
    pub timeout: Option<u64>,
    pub request_interval: Option<u64>,
    pub max_gap: Option<u16>,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub slave: Option<u8>,
    /// DL/T 645 电表地址(1~12 位十进制数字); 点表里没有单独写地址的点使用它
    #[serde(default)]
    pub meter_address: Option<String>,
    pub serial_tty: Option<String>,
    pub baud_rate: Option<u32>,
    pub data_bits: Option<u8>,
    pub parity: Option<String>,
    pub stop_bits: Option<u8>,
    pub interface: Option<String>,
    pub desc: Option<String>,
    /// IEC 61850 的 ISO 层地址参数(AP-Title、选择器), 不填则用库默认值
    #[serde(default)]
    pub iso: Option<IsoConfig>,
}

/// IEC 61850 MMS 连接的 ISO 层地址参数, 数值取自 ICD 里该 IED 的 `<Address>` 段。
///
/// `remote_*` 是对端 IED 的地址, `local_*` 是采集器自己的; 没填的项保持库的默认值。
/// 选择器是十六进制字节串(`"00 01"`), AP-Title 是空格或点分隔的 OID(`"1 3 9999 106"`)。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct IsoConfig {
    pub remote_ap_title: Option<String>,
    pub remote_ae_qualifier: Option<i32>,
    pub remote_psel: Option<String>,
    pub remote_ssel: Option<String>,
    pub remote_tsel: Option<String>,
    pub local_ap_title: Option<String>,
    pub local_ae_qualifier: Option<i32>,
    pub local_psel: Option<String>,
    pub local_ssel: Option<String>,
    pub local_tsel: Option<String>,
}

impl From<IsoConfig> for collector_iec61850::IsoParams {
    fn from(c: IsoConfig) -> Self {
        Self {
            remote_ap_title: c.remote_ap_title,
            remote_ae_qualifier: c.remote_ae_qualifier,
            remote_psel: c.remote_psel,
            remote_ssel: c.remote_ssel,
            remote_tsel: c.remote_tsel,
            local_ap_title: c.local_ap_title,
            local_ae_qualifier: c.local_ae_qualifier,
            local_psel: c.local_psel,
            local_ssel: c.local_ssel,
            local_tsel: c.local_tsel,
        }
    }
}

#[derive(Debug, Clone)]
pub enum ProtocolConfigs {
    Modbus(modbus_conf::ModbusConfigs),
    Iec104(iec104_conf::Iec104Configs),
    Iec61850(iec61850_conf::Iec61850Configs),
    Dlt645(dlt645_conf::Dlt645Configs),
    #[cfg(target_os = "linux")]
    CAN(can_conf::CanConfigs),
    #[cfg(target_os = "linux")]
    GPIO(gpio_conf::GpioConfigs),
    None,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MqttRoute {
    pub device_id: String,
    pub rules: Vec<MqttRule>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MqttRule {
    pub topic: String,
    pub point_ids: Vec<PointId>,
}

pub(crate) fn required_f64(row: &[Data], idx: usize, field: &str) -> Result<f64, anyhow::Error> {
    row.get(idx)
        .and_then(|cell| cell.get_float())
        .ok_or_else(|| anyhow::Error::msg(format!("{field}不能为空")))
}

pub(crate) fn required_str<'a>(
    row: &'a [Data],
    idx: usize,
    field: &str,
) -> Result<&'a str, anyhow::Error> {
    row.get(idx)
        .and_then(|cell| cell.get_string())
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
    row.get(idx).and_then(|cell| cell.get_string()).map(|s| {
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
    let Some(cell) = row.get(idx) else {
        return Err(anyhow::Error::msg(format!("{field}不能为空")));
    };
    if let Some(v) = cell.get_int() {
        return usize::try_from(v).map_err(|_| anyhow::Error::msg(format!("{field}超出范围")));
    }
    if let Some(v) = cell.get_float() {
        if !v.is_finite() || v.fract().abs() > f64::EPSILON {
            return Err(anyhow::Error::msg(format!("{field}必须是整数")));
        }
        return Ok(v as usize);
    }
    Err(anyhow::Error::msg(format!("{field}不能为空")))
}

pub(crate) fn required_hex(row: &[Data], idx: usize, field: &str) -> Result<u32, anyhow::Error> {
    let str = row
        .get(idx)
        .and_then(|cell| cell.get_string())
        .ok_or_else(|| anyhow::Error::msg(format!("{field}不能为空")))?
        .trim();
    let str = str.strip_prefix("0x").unwrap_or(str);
    u32::from_str_radix(str, 16).map_err(|_| anyhow::Error::msg(format!("{field}格式错误")))
}
