use std::collections::{HashMap, HashSet};

use calamine::{Data, DataType, HeaderRow, Range, Reader, Xlsx, open_workbook};
use tracing::error;

use crate::{
    config::{
        optional_static_str, required_f64, required_static_str, required_str,
        required_usize_integerish,
    },
    core::point::{Bits, Translator, WarnLevel, Words},
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
    #[error("Failed to open workbook: {0}")]
    OpenWorkbookError(#[from] calamine::XlsxError),
    #[error("存在重复点位ID: {0}")]
    DuplicatePointId(u16),
}

/// 一行模板配置按"重复次数/地址步长/序号步长"展开为多份（如一堆多簇场景）。
/// 缺少这三列（或重复次数<=1）时原样返回单行，兼容旧配置文件。
fn expand_row(row: &[Data]) -> Vec<Vec<Data>> {
    const REPEAT_COL: usize = 16;
    const ADDR_STEP_COL: usize = 17;
    const ID_STEP_COL: usize = 18;

    let repeat_count = row
        .get(REPEAT_COL)
        .and_then(|cell| cell.get_float())
        .map(|v| v as i64)
        .filter(|&v| v > 1);
    let Some(repeat_count) = repeat_count else {
        return vec![row.to_vec()];
    };

    let steps = row
        .get(0)
        .and_then(|cell| cell.get_float())
        .zip(row.get(5).and_then(|cell| cell.get_float()))
        .zip(row.get(ID_STEP_COL).and_then(|cell| cell.get_float()))
        .zip(row.get(ADDR_STEP_COL).and_then(|cell| cell.get_float()));
    let Some((((base_id, base_addr), id_step), addr_step)) = steps else {
        error!("配置了重复次数({repeat_count})但缺少序号/寄存器地址/序号步长/地址步长，按不展开处理");
        return vec![row.to_vec()];
    };

    (0..repeat_count)
        .map(|i| {
            let n = i + 1; // 簇号从1开始
            let mut new_row = row.to_vec();
            if let Some(cell) = new_row.get_mut(0) {
                *cell = Data::Float(base_id + i as f64 * id_step);
            }
            if let Some(cell) = new_row.get_mut(5) {
                *cell = Data::Float(base_addr + i as f64 * addr_step);
            }
            for (idx, cell) in new_row.iter_mut().enumerate() {
                if idx == 0 || idx == 5 {
                    continue;
                }
                if let Data::String(s) = cell {
                    if s.contains("{n}") {
                        *cell = Data::String(s.replace("{n}", &n.to_string()));
                    }
                }
            }
            new_row
        })
        .collect()
}

pub(crate) fn build_configs(path: String) -> Result<ModbusConfigs, ModbusConfigsError> {
    let mut workbook: Xlsx<_> = open_workbook(&path)?;
    let mut configs = Vec::new();
    let mut errors: HashMap<(&'static str, String), Vec<u32>> = HashMap::new();
    let parse = |sheet: &'static str,
                 range: Range<Data>,
                 configs: &mut Vec<ModbusConfig>,
                 errors: &mut HashMap<(&'static str, String), Vec<u32>>| {
        // Excel行号（1-based，与表格软件里看到的行号一致）
        let row_offset = range.start().map(|(row, _)| row).unwrap_or(0);
        for (idx, row) in range.rows().enumerate() {
            let excel_row = row_offset + idx as u32 + 1;
            for expanded in expand_row(row) {
                match ModbusConfig::build(&expanded) {
                    Ok(config) => {
                        configs.push(config);
                    }
                    Err(err) => {
                        errors
                            .entry((sheet, err.to_string()))
                            .or_default()
                            .push(excel_row);
                    }
                }
            }
        }
    };
    for sheet in ["遥信", "遥控", "遥测", "遥调"] {
        if let Ok(range) = workbook
            .with_header_row(HeaderRow::Row(1))
            .worksheet_range(sheet)
        {
            parse(sheet, range, &mut configs, &mut errors);
        }
    }
    for ((sheet, reason), rows) in &errors {
        let rows_str = rows
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        error!(
            "构建Modbus配置失败[配置文件: {path}][sheet: {sheet}] x{}: {reason}（行: {rows_str}）",
            rows.len()
        );
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
    pub enable: bool,
    pub key: &'static str,
    pub trans: Option<&'static Translator>,
    pub status_words: Option<&'static Words>,
    pub warn_bits: Option<&'static Bits>,
    /// 单点告警等级：用于"单个遥信点位本身就是一个告警"的场景（值非0即命中该等级告警）
    pub level: Option<WarnLevel>,
}

impl ModbusConfig {
    fn build(row: &[Data]) -> Result<Self, anyhow::Error> {
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

        let byte_order =
            ByteOrder::try_from(row.get(8).and_then(|cell| cell.get_string())).ok();
        let scale = required_f64(row, 9, "缩放")?;
        let offset = required_f64(row, 10, "偏移量")?;
        let enable = row
            .get(11)
            .and_then(|cell| cell.get_float())
            .unwrap_or(1f64)
            != 0f64;
        let key = optional_static_str(row, 12).unwrap_or("");
        let trans = row
            .get(13)
            .and_then(|cell| cell.get_string())
            .and_then(|str| Translator::try_from(str).ok());
        let trans: Option<&'static Translator> = match trans {
            Some(t) => Some(Box::leak(Box::new(t))),
            None => None,
        };
        let status_words = row
            .get(14)
            .and_then(|it| it.get_string().and_then(|str| Words::try_from(str).ok()));
        let status_words: Option<&'static Words> = match status_words {
            Some(t) => Some(Box::leak(Box::new(t))),
            None => None,
        };
        let warn_bits = row
            .get(15)
            .and_then(|it| it.get_string().and_then(|str| Bits::try_from(str).ok()));
        let warn_bits: Option<&'static Bits> = match warn_bits {
            Some(t) => Some(Box::leak(Box::new(t))),
            None => None,
        };
        // 索引19：告警等级，追加在重复次数(16)/地址步长(17)/序号步长(18)之后
        let level = row
            .get(19)
            .and_then(|cell| cell.get_float())
            .map(|v| WarnLevel::from(v as u8))
            .filter(|lvl| *lvl != WarnLevel::None);
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
            enable,
            key,
            trans,
            status_words,
            warn_bits,
            level,
        })
    }
}

#[cfg(test)]
mod expand_row_tests {
    use super::*;

    fn base_row() -> Vec<Data> {
        vec![
            Data::Float(50.0),                        // 0: 序号
            Data::String("簇{n}电压".to_string()),      // 1: 点位名称
            Data::String("U16".to_string()),           // 2: 数据类型
            Data::Empty,                               // 3: 单位
            Data::Empty,                               // 4: 备注
            Data::Float(2203.0),                       // 5: 寄存器地址
            Data::String("HoldingRegisters".to_string()), // 6: 寄存器类型
            Data::Float(1.0),                          // 7: 数量
            Data::Empty,                               // 8: 字节序
            Data::Float(1.0),                          // 9: 缩放
            Data::Float(0.0),                           // 10: 偏移量
            Data::Float(1.0),                          // 11: 启用
            Data::String("clusterVoltage{n}".to_string()), // 12: 键
            Data::Empty,                               // 13: 点位名称翻译
            Data::Empty,                               // 14: 状态字
            Data::Empty,                               // 15: 告警位
        ]
    }

    #[test]
    fn expand_row_without_extra_columns_returns_single_row() {
        let row = base_row();
        let expanded = expand_row(&row);
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0], row);
    }

    #[test]
    fn expand_row_with_repeat_count_one_returns_single_row() {
        let mut row = base_row();
        row.push(Data::Float(1.0)); // 16: 重复次数
        row.push(Data::Float(46.0)); // 17: 地址步长
        row.push(Data::Float(1.0)); // 18: 序号步长
        let expanded = expand_row(&row);
        assert_eq!(expanded.len(), 1);
    }

    #[test]
    fn expand_row_generates_n_rows_with_incrementing_id_and_address() {
        let mut row = base_row();
        row.push(Data::Float(12.0)); // 16: 重复次数
        row.push(Data::Float(46.0)); // 17: 地址步长
        row.push(Data::Float(1.0)); // 18: 序号步长

        let expanded = expand_row(&row);
        assert_eq!(expanded.len(), 12);

        assert_eq!(expanded[0][0], Data::Float(50.0));
        assert_eq!(expanded[0][5], Data::Float(2203.0));
        assert_eq!(expanded[0][1], Data::String("簇1电压".to_string()));
        assert_eq!(
            expanded[0][12],
            Data::String("clusterVoltage1".to_string())
        );

        assert_eq!(expanded[11][0], Data::Float(61.0));
        assert_eq!(expanded[11][5], Data::Float(2709.0));
        assert_eq!(expanded[11][1], Data::String("簇12电压".to_string()));
        assert_eq!(
            expanded[11][12],
            Data::String("clusterVoltage12".to_string())
        );
    }

    #[test]
    fn expand_row_missing_steps_falls_back_to_single_row() {
        let mut row = base_row();
        row.push(Data::Float(12.0)); // 16: 重复次数
        // 缺少地址步长(17)/序号步长(18)
        let expanded = expand_row(&row);
        assert_eq!(expanded.len(), 1);
        assert_eq!(expanded[0], row);
    }
}
