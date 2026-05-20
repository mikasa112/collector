use std::path::Path;

use calamine::{Data, DataType, HeaderRow, Reader, Xlsx, open_workbook};

use crate::{
    config::{
        modbus_conf::{ByteOrder, ModbusDataType, RegisterType},
        required_f64, required_str,
    },
    core::point::Val,
};

pub(crate) enum RegValue {
    Bool(bool),
    Word(u16),
    DWord([u16; 2]),
}

pub(crate) struct PointSource {
    pub(crate) source: String,
    pub(crate) point_id: u32,
    #[allow(dead_code)]
    pub(crate) point_key: String,
}

pub(crate) struct NorthboundConfig {
    pub(crate) register_address: u16,
    pub(crate) name: String,
    pub(crate) register_type: RegisterType,
    pub(crate) data_type: ModbusDataType,
    pub(crate) scale: f64,
    pub(crate) offset: f64,
    pub(crate) byte_order: Option<ByteOrder>,
    pub(crate) point_source: PointSource,
}

impl NorthboundConfig {
    pub fn new(row: &[Data]) -> Result<Self, anyhow::Error> {
        let register_address = required_f64(row, 0, "寄存器地址")? as u16;
        let name = required_str(row, 1, "名称")?.to_string();
        let register_type = RegisterType::try_from(required_str(row, 2, "寄存器")?)?;
        let data_type = ModbusDataType::try_from(required_str(row, 3, "数据类型")?)?;
        let byte_order = ByteOrder::try_from(row.get(4).and_then(|d| d.get_string())).ok();
        let scale = required_f64(row, 5, "系数")?;
        let offset = required_f64(row, 6, "偏移量")?;
        let source_str = required_str(row, 7, "来源")?;
        let point_id = required_f64(row, 8, "点位")? as u32;
        let point_key = required_str(row, 9, "键")?;
        let point_source = PointSource {
            source: source_str.to_string(),
            point_id,
            point_key: point_key.to_string(),
        };
        Ok(NorthboundConfig {
            register_address,
            name,
            register_type,
            data_type,
            scale,
            offset,
            byte_order,
            point_source,
        })
    }

    /// 将寄存器原始值还原为工程值（北向写入时使用）
    pub fn restore_val(&self, value: u16) -> Val {
        let raw = (value as f64 - self.offset) / self.scale;
        if raw.is_finite() && (raw.fract().abs() < f64::EPSILON) {
            if raw < 0.0f64 {
                Val::I16(raw as i16)
            } else {
                Val::U16(raw as u16)
            }
        } else {
            Val::F64(raw)
        }
    }

    /// 将工程值编码为寄存器值（DataCenter → 北向表格）
    pub fn encode_val(&self, val: &Val) -> Option<RegValue> {
        match self.data_type {
            ModbusDataType::Bool => {
                let b = bool::try_from(val).ok()?;
                Some(RegValue::Bool(b))
            }
            ModbusDataType::U16 | ModbusDataType::I16 => {
                let raw = f64::try_from(val).ok()?;
                let scaled = (raw * self.scale + self.offset) as u16;
                let word = self.byte_order.map_or(scaled, |bo| bo.assemble_u16(scaled));
                Some(RegValue::Word(word))
            }
            ModbusDataType::U32 => {
                let raw = f64::try_from(val).ok()?;
                let scaled = (raw * self.scale + self.offset) as u32;
                let bo = self.byte_order.unwrap_or(ByteOrder::ABCD);
                Some(RegValue::DWord(bo.assemble_u32(scaled)))
            }
            ModbusDataType::I32 => {
                let raw = f64::try_from(val).ok()?;
                let scaled = (raw * self.scale + self.offset) as i32 as u32;
                let bo = self.byte_order.unwrap_or(ByteOrder::ABCD);
                Some(RegValue::DWord(bo.assemble_u32(scaled)))
            }
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum NorthboundConfigsError {
    #[error("Failed to open workbook: {0}")]
    OpenWorkbookError(#[from] calamine::XlsxError),
}

pub(crate) type NorthboundConfigs = Vec<NorthboundConfig>;

pub(crate) fn build_configs<P: AsRef<Path>>(
    path: P,
) -> Result<NorthboundConfigs, NorthboundConfigsError> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let sheet = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("北向")?;
    let mut configs = Vec::new();
    for row in sheet.rows() {
        match NorthboundConfig::new(row) {
            Ok(config) => configs.push(config),
            Err(err) => {
                tracing::error!("构建北向Modbus配置失败: {}", err);
            }
        }
    }
    Ok(configs)
}
