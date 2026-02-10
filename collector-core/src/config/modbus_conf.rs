use calamine::{Data, DataType, HeaderRow, Range, Reader, Xlsx, open_workbook};
use tracing::error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ModbusDataType {
    Bool,
    U16,
    I16,
    U32,
    I32,
}

impl ModbusDataType {
    pub fn quantity(&self) -> u16 {
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
    if let Ok(range) = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("遥信")
    {
        parse(range, &mut configs);
    }
    if let Ok(range) = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("遥控")
    {
        parse(range, &mut configs);
    }
    if let Ok(range) = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("遥测")
    {
        parse(range, &mut configs);
    }
    if let Ok(range) = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("遥调")
    {
        parse(range, &mut configs);
    }
    Ok(configs)
}

#[derive(Debug, Clone)]
pub struct ModbusConfig {
    pub id: u32,
    pub name: String,
    pub data_type: ModbusDataType,
    pub unit: Option<String>,
    pub remarks: Option<String>,
    pub register_address: u16,
    pub register_type: RegisterType,
    pub byte_order: Option<ByteOrder>,
    pub scale: f64,
    pub offset: f64,
}

impl ModbusConfig {
    fn build(row: &[Data]) -> Result<Self, anyhow::Error> {
        if row.len() != 10 {
            return Err(anyhow::Error::msg("行数据长度不正确"));
        }
        let id = row[0]
            .get_float()
            .ok_or(anyhow::Error::msg("序号不能为空"))? as u32;
        if id >= (1 << 24) {
            return Err(anyhow::Error::msg("序号(id)超出允许范围(0..2^24-1)"));
        }
        let name = row[1]
            .get_string()
            .ok_or(anyhow::Error::msg("点位名称不能为空"))?
            .to_string();
        let data_type = ModbusDataType::try_from(
            row[2]
                .get_string()
                .ok_or(anyhow::Error::msg("数据类型不能为空"))?,
        )?;
        let unit = row[3].get_string().map(str::to_string);
        let remarks = row[4].get_string().map(str::to_string);
        let register_address = row[5]
            .get_float()
            .ok_or(anyhow::Error::msg("寄存器地址不能为空"))? as u16;
        let register_type = RegisterType::try_from(
            row[6]
                .get_string()
                .ok_or(anyhow::Error::msg("寄存器类型不能为空"))?,
        )?;
        let byte_order = ByteOrder::try_from(row[7].get_string()).ok();
        let scale = row[8]
            .get_float()
            .ok_or(anyhow::Error::msg("缩放不能为空"))?;
        let offset = row[9]
            .get_float()
            .ok_or(anyhow::Error::msg("偏移量不能为空"))?;
        Ok(ModbusConfig {
            id,
            name,
            data_type,
            unit,
            remarks,
            register_address,
            register_type,
            byte_order,
            scale,
            offset,
        })
    }

    pub fn serial_num(&self) -> u64 {
        let num = (self.register_type as u32) << 24 | self.id;
        num as u64
    }
}
