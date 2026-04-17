use std::{collections::HashMap, time::Duration};

use calamine::{Data, HeaderRow, Reader, Xlsx, open_workbook};

use crate::config::{
    required_f64, required_hex, required_static_str, required_str, required_usize_integerish,
};

#[derive(Debug, thiserror::Error)]
pub enum CanConfParseError {
    #[error("{entity}列数不正确，期望 {expected}，实际 {actual}")]
    InvalidRowLength {
        entity: &'static str,
        expected: usize,
        actual: usize,
    },
    #[error("{entity}字段 {field} 解析失败: {source}")]
    InvalidField {
        entity: &'static str,
        field: &'static str,
        #[source]
        source: anyhow::Error,
    },
    #[error("{field}值非法: {value}，期望 {expected}")]
    InvalidEnumValue {
        field: &'static str,
        value: String,
        expected: &'static str,
    },
    #[error("打开 CAN 配置工作簿失败")]
    OpenWorkbook(#[from] calamine::XlsxError),
}

impl CanConfParseError {
    const fn invalid_row_length(entity: &'static str, expected: usize, actual: usize) -> Self {
        Self::InvalidRowLength {
            entity,
            expected,
            actual,
        }
    }

    fn invalid_field(entity: &'static str, field: &'static str, source: anyhow::Error) -> Self {
        Self::InvalidField {
            entity,
            field,
            source,
        }
    }

    fn invalid_enum(field: &'static str, value: &str, expected: &'static str) -> Self {
        Self::InvalidEnumValue {
            field,
            value: value.to_owned(),
            expected,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdType {
    Standard,
    Extended,
}

impl TryFrom<&str> for IdType {
    type Error = CanConfParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_lowercase().as_str() {
            "standard" => Ok(IdType::Standard),
            "extended" => Ok(IdType::Extended),
            _ => Err(CanConfParseError::invalid_enum(
                "ID类型",
                value,
                "standard|extended",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    Cycle,
    Trigger,
}

impl TryFrom<&str> for Rule {
    type Error = CanConfParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_lowercase().as_str() {
            "cycle" => Ok(Rule::Cycle),
            "trigger" => Ok(Rule::Trigger),
            _ => Err(CanConfParseError::invalid_enum(
                "规则",
                value,
                "cycle|trigger",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Motorola,
    Intel,
}

impl TryFrom<&str> for ByteOrder {
    type Error = CanConfParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_lowercase().as_str() {
            "motorola" => Ok(ByteOrder::Motorola),
            "intel" => Ok(ByteOrder::Intel),
            _ => Err(CanConfParseError::invalid_enum(
                "字节序",
                value,
                "motorola|intel",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanDataType {
    U8,
    U16,
    I16,
    U32,
    I32,
}

impl TryFrom<&str> for CanDataType {
    type Error = CanConfParseError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.to_lowercase().as_str() {
            "u8" => Ok(CanDataType::U8),
            "u16" => Ok(CanDataType::U16),
            "i16" => Ok(CanDataType::I16),
            "u32" => Ok(CanDataType::U32),
            "i32" => Ok(CanDataType::I32),
            _ => Err(CanConfParseError::invalid_enum(
                "数据类型",
                value,
                "u8|u16|i16|u32|i32",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CanFrameConfig {
    pub id: u32,
    pub name: &'static str,
    pub frame_id: u32,
    pub id_type: IdType,
    pub dlc: u8,
    pub cycle_duration: Duration,
    pub timeout_duration: Duration,
    pub send: &'static str,
    pub receive: &'static str,
    pub rule: Rule,
    pub enable: bool,
}

impl CanFrameConfig {
    fn new(row: &[Data]) -> Result<Self, CanConfParseError> {
        const ENTITY: &str = "CAN报文配置";
        if row.len() != 11 {
            return Err(CanConfParseError::invalid_row_length(ENTITY, 11, row.len()));
        }
        let id = required_usize_integerish(row, 0, "序号")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "序号", err))?
            as u32;
        let name = required_static_str(row, 1, "报文名称")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "报文名称", err))?;
        let frame_id = required_hex(row, 2, "FrameID")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "FrameID", err))?;
        let id_type = IdType::try_from(
            required_str(row, 3, "ID类型")
                .map_err(|err| CanConfParseError::invalid_field(ENTITY, "ID类型", err))?,
        )?;
        let dlc = required_usize_integerish(row, 4, "DLC")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "DLC", err))?
            as u8;
        let cycle_duration = Duration::from_millis(
            required_usize_integerish(row, 5, "周期ms")
                .map_err(|err| CanConfParseError::invalid_field(ENTITY, "周期ms", err))?
                as u64,
        );
        let timeout_duration = Duration::from_millis(
            required_usize_integerish(row, 6, "超时ms")
                .map_err(|err| CanConfParseError::invalid_field(ENTITY, "超时ms", err))?
                as u64,
        );
        let send = required_static_str(row, 7, "发送方")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "发送方", err))?;
        let receive = required_static_str(row, 8, "接收方")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "接收方", err))?;
        let rule = Rule::try_from(
            required_str(row, 9, "规则")
                .map_err(|err| CanConfParseError::invalid_field(ENTITY, "规则", err))?,
        )?;
        let enable = required_usize_integerish(row, 10, "使能")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "使能", err))?
            != 0;
        Ok(Self {
            id,
            name,
            frame_id,
            id_type,
            dlc,
            cycle_duration,
            timeout_duration,
            send,
            receive,
            rule,
            enable,
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct CanSignalConfig {
    pub id: u32,
    pub name: &'static str,
    pub frame_id: u32,
    pub signal_name: &'static str,
    pub start_bit: u8,
    pub bit_len: u8,
    pub byte_order: ByteOrder,
    pub data_type: CanDataType,
    pub scale: f64,
    pub offset: f64,
    pub unit: &'static str,
    pub invalid_val: Option<u32>,
    pub enum_values: &'static str,
}

impl CanSignalConfig {
    fn new(row: &[Data]) -> Result<Self, CanConfParseError> {
        const ENTITY: &str = "CAN信号配置";
        if row.len() != 13 {
            return Err(CanConfParseError::invalid_row_length(ENTITY, 13, row.len()));
        }
        let id = required_usize_integerish(row, 0, "点号")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "点号", err))?
            as u32;
        let name = required_static_str(row, 1, "点位名称")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "点位名称", err))?;
        let frame_id = required_hex(row, 2, "FrameID")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "FrameID", err))?;
        let signal_name = required_static_str(row, 3, "信号名称").unwrap_or_default();
        // .map_err(|err| CanConfParseError::invalid_field(ENTITY, "信号名称", err))?;
        let start_bit = required_usize_integerish(row, 4, "起始位")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "起始位", err))?
            as u8;
        let bit_len = required_usize_integerish(row, 5, "位长")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "位长", err))?
            as u8;
        let byte_order = ByteOrder::try_from(
            required_str(row, 6, "字节序")
                .map_err(|err| CanConfParseError::invalid_field(ENTITY, "字节序", err))?,
        )?;
        let data_type = CanDataType::try_from(
            required_str(row, 7, "数据类型")
                .map_err(|err| CanConfParseError::invalid_field(ENTITY, "数据类型", err))?,
        )?;
        let scale = required_f64(row, 8, "缩放")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "缩放", err))?;
        let offset = required_f64(row, 9, "偏移")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "偏移", err))?;
        let unit = required_static_str(row, 10, "单位").unwrap_or_default();
        // .map_err(|err| CanConfParseError::invalid_field(ENTITY, "单位", err))?;
        let invalid_val = required_hex(row, 11, "无效值").unwrap_or_default();
        // .map_err(|err| CanConfParseError::invalid_field(ENTITY, "无效值", err))?
        // as u32;
        let enum_values = required_static_str(row, 12, "枚举值").unwrap_or_default();
        // .map_err(|err| CanConfParseError::invalid_field(ENTITY, "枚举值", err))?;
        Ok(Self {
            id,
            name,
            frame_id,
            signal_name,
            start_bit,
            bit_len,
            byte_order,
            data_type,
            scale,
            offset,
            unit,
            invalid_val: Some(invalid_val),
            enum_values,
        })
    }
}
#[derive(Debug, Clone, Copy)]
pub struct CanSignalExtConfig {
    pub id: u32,
    pub name: &'static str,
    pub poly_name: &'static str,
    pub frame_id: u32,
    pub frame_num: u16,
    pub frame_id_step: u8,
    pub each_frame_element: u8,
    pub total_element: u16,
    pub element_start_bit: u8,
    pub single_ele_bit_len: u8,
    pub byte_order: ByteOrder,
    pub data_type: CanDataType,
    pub scale: f64,
    pub offset: f64,
    pub unit: &'static str,
    pub invalid_val: Option<u32>,
}

impl CanSignalExtConfig {
    fn new(row: &[Data]) -> Result<Self, CanConfParseError> {
        const ENTITY: &str = "CAN扩展信号配置";
        if row.len() != 16 {
            return Err(CanConfParseError::invalid_row_length(ENTITY, 16, row.len()));
        }
        let id = required_usize_integerish(row, 0, "点号")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "点号", err))?
            as u32;
        let name = required_static_str(row, 1, "点位名称")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "点位名称", err))?;
        let poly_name = required_static_str(row, 2, "聚合键")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "聚合键", err))?;
        let frame_id = required_hex(row, 3, "FrameID")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "FrameID", err))?;
        let frame_num = required_usize_integerish(row, 4, "Frame数量")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "Frame数量", err))?
            as u16;
        let frame_id_step = required_usize_integerish(row, 5, "FrameID步长")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "FrameID步长", err))?
            as u8;
        let each_frame_element = required_usize_integerish(row, 6, "每帧元素")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "每帧元素", err))?
            as u8;
        let total_element = required_usize_integerish(row, 7, "总元素数")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "总元素数", err))?
            as u16;
        let element_start_bit = required_usize_integerish(row, 8, "元素起始Bit")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "元素起始Bit", err))?
            as u8;
        let single_ele_bit_len = required_usize_integerish(row, 9, "单元素BitLen")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "单元素BitLen", err))?
            as u8;
        let byte_order = ByteOrder::try_from(
            required_str(row, 10, "字节序")
                .map_err(|err| CanConfParseError::invalid_field(ENTITY, "字节序", err))?,
        )?;
        let data_type = CanDataType::try_from(
            required_str(row, 11, "数据类型")
                .map_err(|err| CanConfParseError::invalid_field(ENTITY, "数据类型", err))?,
        )?;
        let scale = required_f64(row, 12, "缩放")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "缩放", err))?;
        let offset = required_f64(row, 13, "偏移")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "偏移", err))?;
        let unit = required_static_str(row, 14, "单位")
            .map_err(|err| CanConfParseError::invalid_field(ENTITY, "单位", err))?;
        let invalid_val = required_hex(row, 15, "无效值").unwrap_or_default();
        // .map_err(|err| CanConfParseError::invalid_field(ENTITY, "无效值", err))?
        // as u32;
        Ok(Self {
            id,
            name,
            poly_name,
            frame_id,
            frame_num,
            frame_id_step,
            each_frame_element,
            total_element,
            element_start_bit,
            single_ele_bit_len,
            byte_order,
            data_type,
            scale,
            offset,
            unit,
            invalid_val: Some(invalid_val),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum CanSignal {
    Normal(CanSignalConfig),
    Ext(CanSignalExtConfig),
}

#[derive(Debug, Clone)]
pub struct CanConfig {
    pub frame: CanFrameConfig,
    pub signals: Vec<CanSignal>,
}

pub type CanConfigs = Vec<CanConfig>;

impl CanConfig {}

pub fn build_configs(path: String) -> Result<Vec<CanConfig>, CanConfParseError> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let range = workbook.with_header_row(HeaderRow::Row(1));
    let frames = range
        .worksheet_range("报文")?
        .rows()
        .map(CanFrameConfig::new)
        .collect::<Result<Vec<_>, _>>()?;
    let signals = range
        .worksheet_range("信号")?
        .rows()
        .map(CanSignalConfig::new)
        .collect::<Result<Vec<_>, _>>()?;
    let single_exts = range
        .worksheet_range("信号_扩展")?
        .rows()
        .map(CanSignalExtConfig::new)
        .collect::<Result<Vec<_>, _>>()?;

    let mut signal_map: HashMap<u32, Vec<CanSignal>> = HashMap::new();
    for signal in signals {
        signal_map
            .entry(signal.frame_id)
            .or_default()
            .push(CanSignal::Normal(signal));
    }
    for signal in single_exts {
        signal_map
            .entry(signal.frame_id)
            .or_default()
            .push(CanSignal::Ext(signal));
    }

    Ok(frames
        .into_iter()
        .map(|frame| CanConfig {
            signals: signal_map.remove(&frame.frame_id).unwrap_or_default(),
            frame,
        })
        .collect())
}
