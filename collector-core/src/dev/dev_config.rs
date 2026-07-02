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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
