use std::path::Path;

use calamine::{Data, HeaderRow, Reader, Xlsx, open_workbook};

use crate::{
    config::{
        modbus_conf::{ModbusDataType, RegisterType},
        required_f64, required_str,
    },
    core::point::Val,
};

pub(crate) struct PointSource {
    pub(crate) source: String,
    pub(crate) point_id: u32,
    pub(crate) point_key: String,
}

impl PointSource {}

pub(crate) struct NorthboundConfig {
    pub(crate) register_address: u16,
    pub(crate) register_type: RegisterType,
    pub(crate) data_type: ModbusDataType,
    pub(crate) scale: f64,
    pub(crate) offset: f64,
    pub(crate) point_source: PointSource,
}

impl NorthboundConfig {
    pub fn new(row: &[Data]) -> Result<Self, anyhow::Error> {
        let register_address = required_f64(row, 0, "寄存器地址")? as u16;
        let register_type = RegisterType::try_from(required_str(row, 1, "寄存器")?)?;
        let data_type = ModbusDataType::try_from(required_str(row, 2, "数据类型")?)?;
        let scale = required_f64(row, 3, "系数")?;
        let offset = required_f64(row, 4, "偏移量")?;
        let source_str = required_str(row, 5, "来源")?;
        let point_id = required_f64(row, 6, "点位")? as u32;
        let point_key = required_str(row, 7, "键")?;
        let point_source = PointSource {
            source: source_str.to_string(),
            point_id,
            point_key: point_key.to_string(),
        };
        Ok(NorthboundConfig {
            register_address,
            register_type,
            data_type,
            scale,
            offset,
            point_source,
        })
    }

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
}

#[derive(thiserror::Error, Debug)]
pub enum NorthboundConfigsError {
    #[error("Failed to open workbook: {0}")]
    OpenWorkbookError(#[from] calamine::XlsxError),
}

pub type NorthboundConfigs = Vec<NorthboundConfig>;

pub fn build_configs<P: AsRef<Path>>(path: P) -> Result<NorthboundConfigs, NorthboundConfigsError> {
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
