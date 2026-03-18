use std::collections::HashSet;

use calamine::{Data, DataType, HeaderRow, Range, Reader, Xlsx, open_workbook};
use tracing::error;

use crate::config::{
    optional_static_str, required_f64, required_static_str, required_str, required_usize_integerish,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModbusDataType {
    Bool,
    U16,
    I16,
    U32,
    I32,
}

impl ModbusDataType {
    pub fn register_width(&self) -> u16 {
        match self {
            ModbusDataType::I32 | ModbusDataType::U32 => 2,
            _ => 1,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ModbusDataTypeError {
    #[error("Invalid data type")]
    InvalidDataType,
}

impl TryFrom<&str> for ModbusDataType {
    type Error = ModbusDataTypeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "bool" => Ok(ModbusDataType::Bool),
            "Bool" => Ok(ModbusDataType::Bool),
            "U16" => Ok(ModbusDataType::U16),
            "I16" => Ok(ModbusDataType::I16),
            "U32" => Ok(ModbusDataType::U32),
            "I32" => Ok(ModbusDataType::I32),
            _ => Err(ModbusDataTypeError::InvalidDataType),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ByteOrder {
    AB,
    BA,
    ABCD,
    CDAB,
}

#[derive(Debug, thiserror::Error)]
pub enum ByteOrderError {
    #[error("Invalid byte order")]
    InvalidByteOrder,
}

impl TryFrom<Option<&str>> for ByteOrder {
    type Error = ByteOrderError;

    fn try_from(mut value: Option<&str>) -> Result<Self, Self::Error> {
        let str = value.take();
        match str {
            Some("AB") => Ok(ByteOrder::AB),
            Some("BA") => Ok(ByteOrder::BA),
            Some("ABCD") => Ok(ByteOrder::ABCD),
            Some("CDAB") => Ok(ByteOrder::CDAB),
            _ => Err(ByteOrderError::InvalidByteOrder),
        }
    }
}

impl ByteOrder {
    pub fn assemble_u16(&self, v: u16) -> u16 {
        match self {
            ByteOrder::BA => v.swap_bytes(),
            _ => v,
        }
    }

    pub fn assemble_u32(&self, v: u32) -> [u16; 2] {
        let w0 = (v >> 16) as u16;
        let w1 = (v & 0xFFFF) as u16;
        match self {
            ByteOrder::CDAB => [w1, w0],
            _ => [w0, w1],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RegisterType {
    Coils = 1,
    DiscreteInputs = 2,
    HoldingRegisters = 3,
    InputRegisters = 4,
}

#[derive(Debug, thiserror::Error)]
pub enum RegisterTypeError {
    #[error("Invalid register type")]
    InvalidRegisterType,
}

impl TryFrom<&str> for RegisterType {
    type Error = RegisterTypeError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "Coils" => Ok(RegisterType::Coils),
            "DiscreteInputs" => Ok(RegisterType::DiscreteInputs),
            "HoldingRegisters" => Ok(RegisterType::HoldingRegisters),
            "InputRegisters" => Ok(RegisterType::InputRegisters),
            _ => Err(RegisterTypeError::InvalidRegisterType),
        }
    }
}

pub type ModbusConfigs = Vec<ModbusConfig>;

#[derive(Debug, thiserror::Error)]
pub enum ModbusConfigsError {
    #[error("Failed to open workbook")]
    OpenWorkbookError(#[from] calamine::XlsxError),
    #[error("存在重复点位ID: {0}")]
    DuplicatePointId(u16),
}

pub(crate) fn build_configs(path: String) -> Result<ModbusConfigs, ModbusConfigsError> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let mut configs = Vec::new();
    let parse = |range: Range<Data>, configs: &mut Vec<ModbusConfig>| {
        for row in range.rows() {
            let config = ModbusConfig::build(row);
            match config {
                Ok(config) => {
                    configs.push(config);
                }
                Err(err) => {
                    error!("构建Modbus配置失败: {}", err);
                }
            }
        }
    };
    for sheet in ["遥信", "遥控", "遥测", "遥调"] {
        if let Ok(range) = workbook
            .with_header_row(HeaderRow::Row(1))
            .worksheet_range(sheet)
        {
            parse(range, &mut configs);
        }
    }
    let mut seen = HashSet::with_capacity(configs.len());
    for cfg in &configs {
        if !seen.insert(cfg.id) {
            return Err(ModbusConfigsError::DuplicatePointId(cfg.id));
        }
    }
    Ok(configs)
}

#[derive(Debug, Clone, Copy)]
pub struct ModbusConfig {
    pub id: u16,
    pub name: &'static str,
    pub data_type: ModbusDataType,
    pub unit: Option<&'static str>,
    pub remarks: Option<&'static str>,
    pub register_address: u16,
    pub register_type: RegisterType,
    pub quantity: u16,
    pub byte_order: Option<ByteOrder>,
    pub scale: f64,
    pub offset: f64,
}

impl ModbusConfig {
    fn build(row: &[Data]) -> Result<Self, anyhow::Error> {
        if row.len() != 11 {
            return Err(anyhow::Error::msg("行数据长度不正确"));
        }
        let id = required_f64(row, 0, "序号")?;
        if !(0.0..=(u16::MAX as f64)).contains(&id) {
            return Err(anyhow::Error::msg("序号(id)超出允许范围(0..2^16-1)"));
        }
        let id = id as u16;
        let name = required_static_str(row, 1, "点位名称")?;
        let data_type = ModbusDataType::try_from(required_str(row, 2, "数据类型")?)?;
        let unit = optional_static_str(row, 3);
        let remarks = optional_static_str(row, 4);
        let register_address = required_f64(row, 5, "寄存器地址")? as u16;
        let register_type = RegisterType::try_from(required_str(row, 6, "寄存器类型")?)?;
        let quantity = required_usize_integerish(row, 7, "数量")? as u16;
        let item_width = data_type.register_width();
        if quantity == 0 {
            return Err(anyhow::Error::msg("数量必须大于0"));
        }
        if !quantity.is_multiple_of(item_width) {
            return Err(anyhow::Error::msg("数量与数据类型不匹配"));
        }
        if matches!(
            register_type,
            RegisterType::Coils | RegisterType::HoldingRegisters
        ) && quantity != item_width
        {
            return Err(anyhow::Error::msg("可写寄存器点位只能配置为标量"));
        }
        let byte_order = ByteOrder::try_from(row[8].get_string()).ok();
        let scale = required_f64(row, 9, "缩放")?;
        let offset = required_f64(row, 10, "偏移量")?;
        Ok(ModbusConfig {
            id,
            name,
            data_type,
            unit,
            remarks,
            register_address,
            register_type,
            quantity,
            byte_order,
            scale,
            offset,
        })
    }
}
