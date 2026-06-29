use std::{
    collections::HashMap,
    fmt::{Debug, Display},
};

use serde::{Serialize, ser::SerializeSeq};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ValError {
    #[error("Invalid value")]
    InvalidValue,
}

pub type PointId = u32;

/// 数据点引用方式
///
/// 用于在下发控制命令时指定目标数据点
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum PointRef {
    /// 通过数据点 ID 匹配（最快，O(1) 查找）
    Id(PointId),
    /// 通过数据点 Key 匹配（业务友好的字符串标识）
    Key(String),
    /// 通过数据点 Name 匹配（通常是中文描述）
    Name(String),
}

/// 下发数据点
///
/// 用于 dispatch 方法，只包含必要的信息：目标点 + 值
#[derive(Debug, Clone)]
pub struct DownDataPoint {
    /// 目标数据点的引用方式
    pub point: PointRef,
    /// 要设置的值
    pub value: Val,
}

impl DownDataPoint {
    /// 通过 ID 创建下发点
    pub fn by_id(id: PointId, value: Val) -> Self {
        Self {
            point: PointRef::Id(id),
            value,
        }
    }

    /// 通过 Key 创建下发点
    pub fn by_key(key: String, value: Val) -> Self {
        Self {
            point: PointRef::Key(key),
            value,
        }
    }

    /// 通过 Name 创建下发点
    pub fn by_name(name: String, value: Val) -> Self {
        Self {
            point: PointRef::Name(name),
            value,
        }
    }
}

/// 快速创建 [`DownDataPoint`] 的宏
///
/// # 用法
/// ```
/// down!(id: 2001, Val::U16(0x55))
/// down!(key: "voltage", Val::F64(220.0))
/// down!(name: "电压", Val::F64(220.0))
/// ```
#[macro_export]
macro_rules! down {
    (id: $id:expr, $value:expr) => {
        $crate::core::point::DownDataPoint::by_id($id, $value)
    };
    (key: $key:expr, $value:expr) => {
        $crate::core::point::DownDataPoint::by_key($key, $value)
    };
    (name: $name:expr, $value:expr) => {
        $crate::core::point::DownDataPoint::by_name($name, $value)
    };
}

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    U8(u8),
    I8(i8),
    I16(i16),
    I32(i32),
    U16(u16),
    U32(u32),
    F64(f64),
    List(Vec<Val>),
}

fn val_as_bool(value: &Val) -> Result<bool, ValError> {
    match value {
        Val::U8(v) => Ok(*v != 0),
        Val::I8(v) => Ok(*v != 0),
        Val::I16(v) => Ok(*v != 0),
        Val::I32(v) => Ok(*v != 0),
        Val::U16(v) => Ok(*v != 0),
        Val::U32(v) => Ok(*v != 0),
        Val::F64(v) => Ok(v.abs() > f64::EPSILON),
        Val::List(_) => Err(ValError::InvalidValue),
    }
}

fn val_as_f64(value: &Val) -> Result<f64, ValError> {
    match value {
        Val::U8(v) => Ok(*v as f64),
        Val::I8(v) => Ok(*v as f64),
        Val::I16(v) => Ok(*v as f64),
        Val::I32(v) => Ok(*v as f64),
        Val::U16(v) => Ok(*v as f64),
        Val::U32(v) => Ok(*v as f64),
        Val::F64(v) => Ok(*v),
        Val::List(_) => Err(ValError::InvalidValue),
    }
}

fn val_as_u32(value: &Val) -> Result<u32, ValError> {
    match value {
        Val::U8(v) => Ok(*v as u32),
        Val::I8(v) => Ok(*v as u32),
        Val::I16(v) => Ok(*v as u32),
        Val::I32(v) => Ok(*v as u32),
        Val::U16(v) => Ok(*v as u32),
        Val::U32(v) => Ok(*v),
        Val::F64(v) => Ok(*v as u32),
        Val::List(_) => Err(ValError::InvalidValue),
    }
}

impl TryFrom<Val> for bool {
    type Error = ValError;

    fn try_from(value: Val) -> Result<Self, Self::Error> {
        val_as_bool(&value)
    }
}

impl TryFrom<&Val> for bool {
    type Error = ValError;

    fn try_from(value: &Val) -> Result<Self, Self::Error> {
        val_as_bool(value)
    }
}

impl TryFrom<Val> for f64 {
    type Error = ValError;

    fn try_from(value: Val) -> Result<Self, Self::Error> {
        val_as_f64(&value)
    }
}

impl TryFrom<&Val> for f64 {
    type Error = ValError;

    fn try_from(value: &Val) -> Result<Self, Self::Error> {
        val_as_f64(value)
    }
}

impl TryFrom<&Val> for u32 {
    type Error = ValError;

    fn try_from(value: &Val) -> Result<Self, Self::Error> {
        val_as_u32(value)
    }
}

impl TryFrom<Val> for u32 {
    type Error = ValError;

    fn try_from(value: Val) -> Result<Self, Self::Error> {
        val_as_u32(&value)
    }
}

impl Serialize for Val {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Val::U8(v) => serializer.serialize_u8(*v),
            Val::I8(v) => serializer.serialize_i8(*v),
            Val::I16(v) => serializer.serialize_i16(*v),
            Val::I32(v) => serializer.serialize_i32(*v),
            Val::U16(v) => serializer.serialize_u16(*v),
            Val::U32(v) => serializer.serialize_u32(*v),
            Val::F64(v) => serializer.serialize_f64(*v),
            Val::List(items) => {
                let mut seq = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(item)?;
                }
                seq.end()
            }
        }
    }
}

impl Display for Val {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Val::U8(v) => write!(f, "{}", *v),
            Val::I8(v) => write!(f, "{}", *v),
            Val::I16(v) => write!(f, "{}", *v),
            Val::I32(v) => write!(f, "{}", *v),
            Val::U16(v) => write!(f, "{}", *v),
            Val::U32(v) => write!(f, "{}", *v),
            Val::F64(v) => write!(f, "{}", *v),
            Val::List(vals) => {
                write!(f, "[")?;
                for (i, val) in vals.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", val)?;
                }
                write!(f, "]")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct DataPoint {
    pub id: PointId,
    pub key: &'static str,
    pub name: &'static str,
    pub value: Val,
    pub translator: Option<&'static Translator>,
    pub warn_bits: Option<&'static WarnBits>,
    pub status_word: Option<&'static StatusWords>,
    pub unit: Option<&'static str>,
}

impl DataPoint {
    pub fn warning(&self) -> Vec<WarnBit> {
        let Ok(v) = u32::try_from(&self.value) else {
            return vec![];
        };
        let Some(warn_bits) = self.warn_bits else {
            return vec![];
        };
        warn_bits
            .bits
            .iter()
            .enumerate()
            .filter(|(i, _)| (v >> i) & 1 == 1)
            .map(|(_, bit)| *bit)
            .collect()
    }

    pub fn current_status(&self) -> Option<&'static StatusWord> {
        let v = u32::try_from(&self.value).ok()? as u16;
        self.status_word?.words.get(&v)
    }
}

impl Display for DataPoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.name, self.value)
    }
}

#[derive(Debug)]
pub struct DataPoints(pub Vec<DataPoint>);

impl Display for DataPoints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[")?;
        for (idx, point) in self.0.iter().enumerate() {
            if idx > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", point)?;
        }
        write!(f, "]")
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Translator {
    pub en: &'static str,
}

impl TryFrom<&str> for Translator {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let v: Value = serde_json::from_str(value)?;
        let en_str: &'static str = match v["en"].as_str() {
            Some(v) => String::leak(String::from(v)),
            None => return Err(anyhow::anyhow!("en field is missing")),
        };
        Ok(Self { en: en_str })
    }
}

#[derive(Debug, Clone)]
pub struct WarnBits {
    pub bits: [WarnBit; 16],
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum WarnLevel {
    None = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

impl From<u8> for WarnLevel {
    fn from(value: u8) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Normal,
            2 => Self::High,
            3 => Self::Critical,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WarnBit {
    pub zh: &'static str,
    pub en: &'static str,
    pub level: WarnLevel,
}

impl Default for WarnBit {
    fn default() -> Self {
        Self {
            zh: Default::default(),
            en: Default::default(),
            level: WarnLevel::None,
        }
    }
}

impl TryFrom<&str> for WarnBit {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let values = value.split("|").collect::<Vec<_>>();
        let zh = values
            .first()
            .ok_or(anyhow::anyhow!("zh field is missing"))?
            .to_string();
        let en = values
            .get(1)
            .ok_or(anyhow::anyhow!("en field is missing"))?
            .to_string();
        let level = match values.get(2) {
            Some(s) => WarnLevel::from(s.parse::<u8>()?),
            None => WarnLevel::None,
        };
        Ok(Self {
            zh: String::leak(zh),
            en: String::leak(en),
            level,
        })
    }
}

impl TryFrom<&str> for WarnBits {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let values: Vec<&str> = value.trim().lines().map(|it| it.trim()).collect();
        if values.len() != 16 {
            return Err(anyhow::anyhow!("expected 16 values, got {}", values.len()));
        }
        let mut bits = [WarnBit::default(); 16];
        for (i, s) in values.iter().enumerate() {
            bits[i] = WarnBit::try_from(*s)?;
        }
        Ok(WarnBits { bits })
    }
}

#[derive(Debug, Clone)]
pub struct StatusWords {
    pub words: HashMap<u16, StatusWord>,
}

impl TryFrom<&str> for StatusWords {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let values: Vec<&str> = value.trim().lines().map(|s| s.trim()).collect();
        let mut map = HashMap::with_capacity(value.len());
        for value in values.into_iter() {
            let col_strs: Vec<&str> = value.split(' ').collect();
            let word = col_strs
                .first()
                .ok_or(anyhow::anyhow!("`word` field is missing"))?
                .parse::<u16>()?;
            let status = *col_strs
                .get(1)
                .ok_or(anyhow::anyhow!("`status` field is missing"))?;
            let status_word = StatusWord::try_from(status)?;
            map.insert(word, status_word);
        }
        Ok(Self { words: map })
    }
}

#[derive(Debug, Clone)]
pub struct StatusWord {
    pub zh: &'static str,
    pub en: &'static str,
}

impl TryFrom<&str> for StatusWord {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let values = value.split("|").collect::<Vec<_>>();
        let zh = values
            .first()
            .ok_or(anyhow::anyhow!("zh field is missing"))?
            .to_string();
        let en = values
            .get(1)
            .ok_or(anyhow::anyhow!("en field is missing"))?
            .to_string();
        Ok(Self {
            zh: String::leak(zh),
            en: String::leak(en),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_translator() {
        let translator = Translator::try_from(r#"{"en": "Hello, World!"}"#).unwrap();
        assert_eq!(translator.en, "Hello, World!");
    }

    #[test]
    fn test_parse_status_word() {
        let status_word = StatusWord::try_from("zh|en").unwrap();
        assert_eq!(status_word.zh, "zh");
        assert_eq!(status_word.en, "en");
    }

    #[test]
    fn test_parse_status_words() {
        let status_words = StatusWords::try_from("0 zh|en\r\n1 zh|en").unwrap();
        assert_eq!(status_words.words.len(), 2);
        assert_eq!(status_words.words[&0].zh, "zh");
        assert_eq!(status_words.words[&0].en, "en");
        assert_eq!(status_words.words[&1].zh, "zh");
        assert_eq!(status_words.words[&1].en, "en");
    }

    #[test]
    fn test_parse_warn_bit() {
        let warn_bit = WarnBit::try_from("zh|en|1").unwrap();
        assert_eq!(warn_bit.zh, "zh");
        assert_eq!(warn_bit.en, "en");
        assert_eq!(warn_bit.level, WarnLevel::Normal);
    }

    #[test]
    fn test_parse_warn_bits() {
        let warn_bits = WarnBits::try_from(
            r#"硬件故障-A 相硬件过流|Hardware failure - Phase A hardware overcurrent
        硬件故障-B 相硬件过流|Hardware failure - B-phase hardware overcurrent
        硬件故障-C 相硬件过流|Hardware failure - C-phase hardware overcurrent
        硬件故障-N 相硬件过流|Hardware failure - N-phase hardware overcurrent
        硬件故障-交流直流功率不匹配|Hardware malfunction - AC/DC power mismatch
        硬件故障-辅助源掉电|Hardware malfunction - Auxiliary source power failure
        硬件故障-温度过低或传感器故障|Hardware malfunction - low temperature or sensor failure
        硬件故障-保留|Hardware Failure - Reserved
        硬件故障-绝缘故障|Hardware Failure - Insulation Failure
        硬件故障-总驱动故障|Hardware Failure - Total Driver Failure
        硬件故障-A 相驱动故障|Hardware failure - A-phase drive failure
        硬件故障-B 相驱动故障|Hardware failure - B-phase drive failure
        硬件故障-C 相驱动故障|Hardware failure - C-phase drive failure
        硬件故障-N 相驱动故障|Hardware failure - N-phase drive failure
        硬件故障-散热器过温故障|Hardware malfunction - heatsink overheating fault
        硬件故障-环境过温故障|Hardware malfunction - environmental overheating fault"#,
        )
        .unwrap();
        assert_eq!(warn_bits.bits[0].zh, "硬件故障-A 相硬件过流");
        assert_eq!(
            warn_bits.bits[0].en,
            "Hardware failure - Phase A hardware overcurrent"
        );
        assert_eq!(warn_bits.bits[0].level, WarnLevel::None);
        assert_eq!(warn_bits.bits[1].zh, "硬件故障-B 相硬件过流");
        assert_eq!(
            warn_bits.bits[1].en,
            "Hardware failure - B-phase hardware overcurrent"
        );
        assert_eq!(warn_bits.bits[1].level, WarnLevel::None);
    }
}
