use smallvec::SmallVec;

#[derive(Debug, thiserror::Error)]
pub enum ModbusEntryError {
    #[error("无效的功能码:{0}")]
    InvalidFunctionCode(u8),
    #[error("无效的数据类型:{0}")]
    InvalidDataType(String),
}

pub enum ModbusEntryDataType {
    Bit,
    U16,
    I16,
    U32,
    I32,
    F32,
}

impl TryFrom<String> for ModbusEntryDataType {
    type Error = ModbusEntryError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        match value.as_str() {
            "Bit" => Ok(ModbusEntryDataType::Bit),
            "U16" => Ok(ModbusEntryDataType::U16),
            "I16" => Ok(ModbusEntryDataType::I16),
            "U32" => Ok(ModbusEntryDataType::U32),
            "I32" => Ok(ModbusEntryDataType::I32),
            "F32" => Ok(ModbusEntryDataType::F32),
            _ => Err(ModbusEntryError::InvalidDataType(value)),
        }
    }
}

pub enum ModbusEntryFunction {
    ReadCoil,
    ReadDiscreteInput,
    ReadHoldingRegister,
    ReadInputRegister,
    WriteSingleCoil,
    WriteSingleRegister,
    WriteMultipleCoils,
    WriteMultipleRegisters,
}

impl TryFrom<u8> for ModbusEntryFunction {
    type Error = ModbusEntryError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x1 => Ok(ModbusEntryFunction::ReadCoil),
            0x2 => Ok(ModbusEntryFunction::ReadDiscreteInput),
            0x3 => Ok(ModbusEntryFunction::ReadHoldingRegister),
            0x4 => Ok(ModbusEntryFunction::ReadInputRegister),
            0x5 => Ok(ModbusEntryFunction::WriteSingleCoil),
            0x6 => Ok(ModbusEntryFunction::WriteSingleRegister),
            0xF => Ok(ModbusEntryFunction::WriteMultipleCoils),
            0x10 => Ok(ModbusEntryFunction::WriteMultipleRegisters),
            _ => Err(ModbusEntryError::InvalidFunctionCode(value)),
        }
    }
}

pub struct ModbusEntry {
    pub id: u16,
    pub name: String,
    pub data: Option<SmallVec<[u16; 2]>>,
    pub data_type: ModbusEntryDataType,
    pub unit: String,
    pub address: u16,
    pub data_len: u16,
    pub factor: f32,
    pub offset: f32,
    pub function: ModbusEntryFunction,
}
