use std::net::IpAddr;
use std::time::Duration;

use crate::config::DeviceConfig;

#[derive(Debug, thiserror::Error)]
pub enum ModbusTcpConfError {
    #[error("{0}不能为空")]
    ValueNotNone(String),
    #[error("无效的IP:{0}地址")]
    InvalidIp(String),
}

#[derive(Clone)]
pub struct ModbusTcpConfig {
    pub slave: u8,
    pub ip: String,
    pub port: u16,
    pub interval: u64,
    pub timeout: u64,
    pub request_interval: u64,
    pub max_gap: u16,
}

impl TryFrom<DeviceConfig> for ModbusTcpConfig {
    type Error = ModbusTcpConfError;

    fn try_from(value: DeviceConfig) -> Result<Self, Self::Error> {
        let Some(slave) = value.slave else {
            return Err(ModbusTcpConfError::ValueNotNone(String::from("从站地址")));
        };
        let Some(ip) = value.ip else {
            return Err(ModbusTcpConfError::ValueNotNone(String::from("IP")));
        };
        let Some(port) = value.port else {
            return Err(ModbusTcpConfError::ValueNotNone(String::from("端口")));
        };
        let Some(interval) = value.interval else {
            return Err(ModbusTcpConfError::ValueNotNone(String::from("间隔时间")));
        };
        let Some(timeout) = value.timeout else {
            return Err(ModbusTcpConfError::ValueNotNone(String::from("超时时间")));
        };
        if ip.parse::<IpAddr>().is_err() {
            return Err(ModbusTcpConfError::InvalidIp(ip));
        }
        let request_interval = value.request_interval.unwrap_or(0);
        let max_gap = value.max_gap.unwrap_or(0);
        Ok(ModbusTcpConfig {
            slave,
            ip,
            port,
            interval,
            timeout,
            request_interval,
            max_gap,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModbusRtuConfError {
    #[error("{0}不能为空")]
    ValueNotNone(String),
}

#[derive(Clone)]
pub struct ModbusRtuConfig {
    pub slave: u8,
    pub serial_tty: String,
    pub baudrate: u32,
    pub data_bits: u8,
    pub parity: String,
    pub stop_bits: u8,
    pub interval: u64,
    pub timeout: u64,
    pub request_interval: u64,
    pub max_gap: u16,
}

impl TryFrom<DeviceConfig> for ModbusRtuConfig {
    type Error = ModbusRtuConfError;

    fn try_from(value: DeviceConfig) -> Result<Self, Self::Error> {
        let Some(slave) = value.slave else {
            return Err(ModbusRtuConfError::ValueNotNone(String::from("从站地址")));
        };
        let Some(serial_tty) = value.serial_tty else {
            return Err(ModbusRtuConfError::ValueNotNone(String::from("串口设备")));
        };
        let Some(baudrate) = value.baud_rate else {
            return Err(ModbusRtuConfError::ValueNotNone(String::from("波特率")));
        };
        let Some(data_bits) = value.data_bits else {
            return Err(ModbusRtuConfError::ValueNotNone(String::from("数据位")));
        };
        let Some(parity) = value.parity else {
            return Err(ModbusRtuConfError::ValueNotNone(String::from("校验位")));
        };
        let Some(stop_bits) = value.stop_bits else {
            return Err(ModbusRtuConfError::ValueNotNone(String::from("停止位")));
        };
        let Some(interval) = value.interval else {
            return Err(ModbusRtuConfError::ValueNotNone(String::from("间隔时间")));
        };
        let Some(timeout) = value.timeout else {
            return Err(ModbusRtuConfError::ValueNotNone(String::from("超时时间")));
        };
        let request_interval = value.request_interval.unwrap_or(0);
        let max_gap = value.max_gap.unwrap_or(0);
        Ok(ModbusRtuConfig {
            slave,
            serial_tty,
            baudrate,
            data_bits,
            parity,
            stop_bits,
            interval,
            timeout,
            request_interval,
            max_gap,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CanConfError {
    #[error("{0}不能为空")]
    ValueNotNone(String),
}

#[derive(Clone)]
pub struct CanDeviceConfig {
    pub interface: String,
    pub interval: Duration,
    pub timeout: Duration,
    pub bitrate: Option<u32>,
}

impl TryFrom<DeviceConfig> for CanDeviceConfig {
    type Error = CanConfError;

    fn try_from(value: DeviceConfig) -> Result<Self, Self::Error> {
        let Some(interface) = value.interface else {
            return Err(CanConfError::ValueNotNone(String::from("CAN接口")));
        };
        let Some(interval) = value.interval else {
            return Err(CanConfError::ValueNotNone(String::from("间隔时间")));
        };
        let Some(timeout) = value.timeout else {
            return Err(CanConfError::ValueNotNone(String::from("超时时间")));
        };
        Ok(Self {
            interface,
            interval: Duration::from_millis(interval),
            timeout: Duration::from_millis(timeout),
            bitrate: value.baud_rate,
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Iec104ConfError {
    #[error("{0}不能为空")]
    ValueNotNone(String),
    #[error("无效的IP:{0}地址")]
    InvalidIp(String),
}

/// IEC 104 主站（客户端）连接参数。
#[derive(Clone)]
pub struct Iec104DeviceConfig {
    pub ip: String,
    pub port: u16,
    /// 总召唤周期；为 0 表示仅在连接建立后召唤一次
    pub interval: Duration,
    /// 连接超时（t0）
    pub timeout: Duration,
}

impl TryFrom<DeviceConfig> for Iec104DeviceConfig {
    type Error = Iec104ConfError;

    fn try_from(value: DeviceConfig) -> Result<Self, Self::Error> {
        let Some(ip) = value.ip else {
            return Err(Iec104ConfError::ValueNotNone(String::from("IP")));
        };
        let Some(port) = value.port else {
            return Err(Iec104ConfError::ValueNotNone(String::from("端口")));
        };
        let Some(interval) = value.interval else {
            return Err(Iec104ConfError::ValueNotNone(String::from("间隔时间")));
        };
        let Some(timeout) = value.timeout else {
            return Err(Iec104ConfError::ValueNotNone(String::from("超时时间")));
        };
        if ip.parse::<IpAddr>().is_err() {
            return Err(Iec104ConfError::InvalidIp(ip));
        }
        Ok(Self {
            ip,
            port,
            interval: Duration::from_millis(interval),
            timeout: Duration::from_millis(timeout),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Iec61850ConfError {
    #[error("{0}不能为空")]
    ValueNotNone(String),
    #[error("无效的IP:{0}地址")]
    InvalidIp(String),
    #[error("ISO参数无效: {0}")]
    InvalidIso(String),
}

/// IEC 61850 主站（MMS 客户端）连接参数。
#[derive(Clone)]
pub struct Iec61850DeviceConfig {
    pub ip: String,
    pub port: u16,
    /// 周期任务间隔：定期重新触发总召、轮询未被报告覆盖的点；为 0 表示只在连接建立后做一次
    pub interval: Duration,
    /// 连接超时，同时作为单次 MMS 请求的超时
    pub timeout: Duration,
    /// ISO 层地址参数；`None` 使用库默认值
    pub iso: Option<collector_iec61850::IsoParams>,
}

impl TryFrom<DeviceConfig> for Iec61850DeviceConfig {
    type Error = Iec61850ConfError;

    fn try_from(value: DeviceConfig) -> Result<Self, Self::Error> {
        let Some(ip) = value.ip else {
            return Err(Iec61850ConfError::ValueNotNone(String::from("IP")));
        };
        let Some(port) = value.port else {
            return Err(Iec61850ConfError::ValueNotNone(String::from("端口")));
        };
        let Some(interval) = value.interval else {
            return Err(Iec61850ConfError::ValueNotNone(String::from("间隔时间")));
        };
        let Some(timeout) = value.timeout else {
            return Err(Iec61850ConfError::ValueNotNone(String::from("超时时间")));
        };
        if ip.parse::<IpAddr>().is_err() {
            return Err(Iec61850ConfError::InvalidIp(ip));
        }
        let iso: Option<collector_iec61850::IsoParams> = value.iso.map(Into::into);
        if let Some(iso) = &iso {
            iso.validate()
                .map_err(|e| Iec61850ConfError::InvalidIso(e.to_string()))?;
        }
        Ok(Self {
            ip,
            port,
            interval: Duration::from_millis(interval),
            // 太短的超时在真实网络里没有意义, 与 104 一样至少 1 秒
            timeout: Duration::from_millis(timeout).max(Duration::from_secs(1)),
            iso,
        })
    }
}
