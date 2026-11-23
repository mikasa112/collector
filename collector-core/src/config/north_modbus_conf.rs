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
    ///
    /// 系数语义：工程值 = 寄存器值 × 系数 + 偏移量（如系数 0.1：寄存器 500 → 50.0）。
    pub fn restore_val(&self, value: u16) -> Val {
        // I16 的寄存器值按补码还原为有符号数
        let reg = match self.data_type {
            ModbusDataType::I16 => (value as i16) as f64,
            _ => value as f64,
        };
        let raw = reg * self.scale + self.offset;
        // 乘小数系数会引入浮点误差（如 500*0.1），按 1e-9 精度收敛后再判断是否为整数
        let raw = (raw * 1e9).round() / 1e9;
        if raw.is_finite() && raw.fract() == 0.0 {
            if raw < 0.0f64 {
                Val::I16(raw as i16)
            } else {
                Val::U16(raw as u16)
            }
        } else {
            Val::F64(raw)
        }
    }

    /// 工程值 → 寄存器整数值：寄存器值 = (工程值 − 偏移量) ÷ 系数，四舍五入
    fn scale_to_reg(&self, raw: f64) -> f64 {
        ((raw - self.offset) / self.scale).round()
    }

    /// 将工程值编码为寄存器值（DataCenter → 北向表格）
    pub fn encode_val(&self, val: &Val) -> Option<RegValue> {
        match self.data_type {
            ModbusDataType::Bool => {
                let b = bool::try_from(val).ok()?;
                Some(RegValue::Bool(b))
            }
            ModbusDataType::U16 | ModbusDataType::I16 => {
                let v = self.scale_to_reg(f64::try_from(val).ok()?);
                if !v.is_finite() {
                    return None;
                }
                // 负值先转 i16 再按补码位模式转 u16（直接 as u16 会饱和成 0）
                let scaled = if v < 0.0 { (v as i16) as u16 } else { v as u16 };
                let word = self.byte_order.map_or(scaled, |bo| bo.assemble_u16(scaled));
                Some(RegValue::Word(word))
            }
            ModbusDataType::U32 => {
                let v = self.scale_to_reg(f64::try_from(val).ok()?);
                if !v.is_finite() {
                    return None;
                }
                let bo = self.byte_order.unwrap_or(ByteOrder::ABCD);
                Some(RegValue::DWord(bo.assemble_u32(v as u32)))
            }
            ModbusDataType::I32 => {
                let v = self.scale_to_reg(f64::try_from(val).ok()?);
                if !v.is_finite() {
                    return None;
                }
                let bo = self.byte_order.unwrap_or(ByteOrder::ABCD);
                Some(RegValue::DWord(bo.assemble_u32(v as i32 as u32)))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(data_type: ModbusDataType, scale: f64, offset: f64) -> NorthboundConfig {
        NorthboundConfig {
            register_address: 0,
            name: String::new(),
            register_type: RegisterType::InputRegisters,
            data_type,
            scale,
            offset,
            byte_order: Some(ByteOrder::AB),
            point_source: PointSource { source: "t".into(), point_id: 1, point_key: "k".into() },
        }
    }

    fn word(c: &NorthboundConfig, v: f64) -> u16 {
        match c.encode_val(&Val::F64(v)).unwrap() {
            RegValue::Word(w) => w,
            _ => panic!("期望 Word"),
        }
    }

    #[test]
    fn scale_0_1_value_50_is_500() {
        // 系数 0.1：工程值 50 → 寄存器 500（读回 ×0.1 = 50.0）
        assert_eq!(word(&cfg(ModbusDataType::U16, 0.1, 0.0), 50.0), 500);
        assert_eq!(word(&cfg(ModbusDataType::U16, 0.1, 0.0), 49.97), 500);
        assert_eq!(word(&cfg(ModbusDataType::U16, 0.001, 0.0), 1.234), 1234);
        assert_eq!(word(&cfg(ModbusDataType::U16, 1.0, 0.0), 22.0), 22);
    }

    #[test]
    fn negative_i16_is_twos_complement() {
        assert_eq!(word(&cfg(ModbusDataType::I16, 0.1, 0.0), -12.5) as i16, -125);
    }

    #[test]
    fn restore_is_inverse_of_encode() {
        let c = cfg(ModbusDataType::U16, 0.1, 0.0);
        assert!(matches!(c.restore_val(500), Val::U16(50)));
        let c = cfg(ModbusDataType::I16, 0.1, 0.0);
        assert!(matches!(c.restore_val((-125i16) as u16), Val::F64(v) if (v + 12.5).abs() < 1e-9));
    }

    #[test]
    fn dword_scaled() {
        let c = cfg(ModbusDataType::U32, 0.1, 0.0);
        match c.encode_val(&Val::F64(100000.0)).unwrap() {
            RegValue::DWord([hi, lo]) => assert_eq!(((hi as u32) << 16) | lo as u32, 1_000_000),
            _ => panic!(),
        }
    }
}
