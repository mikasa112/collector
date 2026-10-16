use std::ffi::CStr;

use crate::ffi;

/// 功能约束(FC), 决定一个数据属性属于状态、测量、定值还是控制等。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Fc {
    St,
    Mx,
    Sp,
    Sv,
    Cf,
    Dc,
    Sg,
    Se,
    Ex,
    Co,
}

impl Fc {
    pub(crate) fn raw(self) -> ffi::FunctionalConstraint {
        match self {
            Fc::St => ffi::eFunctionalConstraint_IEC61850_FC_ST,
            Fc::Mx => ffi::eFunctionalConstraint_IEC61850_FC_MX,
            Fc::Sp => ffi::eFunctionalConstraint_IEC61850_FC_SP,
            Fc::Sv => ffi::eFunctionalConstraint_IEC61850_FC_SV,
            Fc::Cf => ffi::eFunctionalConstraint_IEC61850_FC_CF,
            Fc::Dc => ffi::eFunctionalConstraint_IEC61850_FC_DC,
            Fc::Sg => ffi::eFunctionalConstraint_IEC61850_FC_SG,
            Fc::Se => ffi::eFunctionalConstraint_IEC61850_FC_SE,
            Fc::Ex => ffi::eFunctionalConstraint_IEC61850_FC_EX,
            Fc::Co => ffi::eFunctionalConstraint_IEC61850_FC_CO,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Fc::St => "ST",
            Fc::Mx => "MX",
            Fc::Sp => "SP",
            Fc::Sv => "SV",
            Fc::Cf => "CF",
            Fc::Dc => "DC",
            Fc::Sg => "SG",
            Fc::Se => "SE",
            Fc::Ex => "EX",
            Fc::Co => "CO",
        }
    }
}

impl std::str::FromStr for Fc {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_uppercase().as_str() {
            "ST" => Fc::St,
            "MX" => Fc::Mx,
            "SP" => Fc::Sp,
            "SV" => Fc::Sv,
            "CF" => Fc::Cf,
            "DC" => Fc::Dc,
            "SG" => Fc::Sg,
            "SE" => Fc::Se,
            "EX" => Fc::Ex,
            "CO" => Fc::Co,
            other => return Err(format!("未知的功能约束 FC: {other}")),
        })
    }
}

/// 品质 `q`(IEC 61850-7-3 Quality), 由库解析 13 位 bit string 得到。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Quality(pub u16);

impl Quality {
    pub const GOOD: Quality = Quality(0);

    /// 有效性: 0 good, 1 reserved, 2 invalid, 3 questionable(与库内部编码一致)。
    pub fn validity(self) -> u16 {
        self.0 & 0x3
    }

    pub fn is_good(self) -> bool {
        self.validity() == ffi::QUALITY_VALIDITY_GOOD as u16
    }

    pub fn is_invalid(self) -> bool {
        self.validity() == ffi::QUALITY_VALIDITY_INVALID as u16
    }

    pub fn is_questionable(self) -> bool {
        self.validity() == ffi::QUALITY_VALIDITY_QUESTIONABLE as u16
    }

    pub fn has(self, flag: u32) -> bool {
        self.0 as u32 & flag != 0
    }

    pub fn overflow(self) -> bool {
        self.has(ffi::QUALITY_DETAIL_OVERFLOW)
    }

    pub fn out_of_range(self) -> bool {
        self.has(ffi::QUALITY_DETAIL_OUT_OF_RANGE)
    }

    pub fn old_data(self) -> bool {
        self.has(ffi::QUALITY_DETAIL_OLD_DATA)
    }

    pub fn failure(self) -> bool {
        self.has(ffi::QUALITY_DETAIL_FAILURE)
    }
}

/// 双点位置 `Dbpos`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dbpos {
    Intermediate,
    Off,
    On,
    BadState,
}

/// 从 MMS 读到的值(已拷贝为 Rust 自有数据, 不再引用 C 内存)。
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// 报告里未包含的元素。
    Null,
    Bool(bool),
    Int(i64),
    Uint(u64),
    Float(f64),
    /// 一般 bit string(如 TrgOps)。`bits` 为库按整数解释的结果, 仅 `size <= 32` 有意义。
    BitString { size: u32, bits: u32 },
    /// 13 位的 bit string 被识别为品质。
    Quality(Quality),
    /// 2 位的 bit string 被识别为双点位置。
    Dbpos(Dbpos),
    String(String),
    Octets(Vec<u8>),
    /// UTC 时间戳, 毫秒(Unix epoch)。
    UtcTime(u64),
    Struct(Vec<Value>),
    Array(Vec<Value>),
    /// 服务端对该元素返回了 DataAccessError(原始错误码)。
    AccessError(u32),
    /// 其他暂未映射的 MMS 类型(原始类型号)。
    Other(u32),
}

impl Value {
    /// 把数值类标量转成 `f64`, 其余类型返回 `None`。
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            Value::Int(v) => Some(*v as f64),
            Value::Uint(v) => Some(*v as f64),
            Value::Float(v) => Some(*v),
            _ => None,
        }
    }

    /// 按名字取结构体成员以外的快捷方式: 从 Struct 里按下标取值。
    pub fn field(&self, index: usize) -> Option<&Value> {
        match self {
            Value::Struct(v) | Value::Array(v) => v.get(index),
            _ => None,
        }
    }

    /// 从 C 端的 `MmsValue` 深拷贝。空指针得到 `Value::Null`。
    ///
    /// # Safety
    /// `ptr` 为空或指向有效的 `MmsValue`, 并且在本调用期间不被其他线程释放。
    pub(crate) unsafe fn from_mms(ptr: *mut ffi::MmsValue) -> Value {
        if ptr.is_null() {
            return Value::Null;
        }
        unsafe {
            match ffi::MmsValue_getType(ptr) {
                ffi::MmsType_MMS_ARRAY | ffi::MmsType_MMS_STRUCTURE => {
                    let is_array = ffi::MmsValue_getType(ptr) == ffi::MmsType_MMS_ARRAY;
                    let n = ffi::MmsValue_getArraySize(ptr);
                    let items = (0..n)
                        .map(|i| Value::from_mms(ffi::MmsValue_getElement(ptr, i as _)))
                        .collect();
                    if is_array {
                        Value::Array(items)
                    } else {
                        Value::Struct(items)
                    }
                }
                ffi::MmsType_MMS_BOOLEAN => Value::Bool(ffi::MmsValue_getBoolean(ptr)),
                ffi::MmsType_MMS_BIT_STRING => {
                    let size = ffi::MmsValue_getBitStringSize(ptr).max(0) as u32;
                    match size {
                        13 => Value::Quality(Quality(ffi::Quality_fromMmsValue(ptr))),
                        2 => Value::Dbpos(match ffi::Dbpos_fromMmsValue(ptr) {
                            ffi::Dbpos_DBPOS_OFF => Dbpos::Off,
                            ffi::Dbpos_DBPOS_ON => Dbpos::On,
                            ffi::Dbpos_DBPOS_BAD_STATE => Dbpos::BadState,
                            _ => Dbpos::Intermediate,
                        }),
                        _ => Value::BitString {
                            size,
                            bits: if size <= 32 {
                                ffi::MmsValue_getBitStringAsInteger(ptr)
                            } else {
                                0
                            },
                        },
                    }
                }
                ffi::MmsType_MMS_INTEGER => Value::Int(ffi::MmsValue_toInt64(ptr)),
                ffi::MmsType_MMS_UNSIGNED => Value::Uint(u64::from(ffi::MmsValue_toUint32(ptr))),
                ffi::MmsType_MMS_FLOAT => Value::Float(ffi::MmsValue_toDouble(ptr)),
                ffi::MmsType_MMS_OCTET_STRING => {
                    let len = ffi::MmsValue_getOctetStringSize(ptr) as usize;
                    let buf = ffi::MmsValue_getOctetStringBuffer(ptr);
                    if buf.is_null() || len == 0 {
                        Value::Octets(Vec::new())
                    } else {
                        Value::Octets(std::slice::from_raw_parts(buf, len).to_vec())
                    }
                }
                ffi::MmsType_MMS_VISIBLE_STRING | ffi::MmsType_MMS_STRING => {
                    let s = ffi::MmsValue_toString(ptr);
                    if s.is_null() {
                        Value::String(String::new())
                    } else {
                        Value::String(CStr::from_ptr(s).to_string_lossy().into_owned())
                    }
                }
                ffi::MmsType_MMS_UTC_TIME => Value::UtcTime(ffi::MmsValue_getUtcTimeInMs(ptr)),
                ffi::MmsType_MMS_BINARY_TIME => {
                    Value::UtcTime(ffi::MmsValue_getBinaryTimeAsUtcMs(ptr))
                }
                ffi::MmsType_MMS_DATA_ACCESS_ERROR => {
                    Value::AccessError(ffi::MmsValue_getDataAccessError(ptr) as u32)
                }
                other => Value::Other(other),
            }
        }
    }
}

/// 一个数据对象/属性的类型结构(只含字段名和形状), 来自服务端的 `GetVariableAccessAttributes`。
#[derive(Debug, Clone, PartialEq)]
pub enum TypeSpec {
    Struct(Vec<(String, TypeSpec)>),
    Array(usize, Box<TypeSpec>),
    /// 叶子, 值是原始 MMS 类型号。
    Leaf(u32),
}

impl TypeSpec {
    /// # Safety
    /// `ptr` 为有效的 `MmsVariableSpecification`, 且在本调用期间不被释放。
    pub(crate) unsafe fn from_mms(ptr: *mut ffi::MmsVariableSpecification) -> TypeSpec {
        unsafe {
            match ffi::MmsVariableSpecification_getType(ptr) {
                ffi::MmsType_MMS_STRUCTURE => {
                    let n = ffi::MmsVariableSpecification_getSize(ptr).max(0);
                    let fields = (0..n)
                        .filter_map(|i| {
                            let child = ffi::MmsVariableSpecification_getChildSpecificationByIndex(ptr, i);
                            if child.is_null() {
                                return None;
                            }
                            let name = ffi::MmsVariableSpecification_getName(child);
                            let name = if name.is_null() {
                                String::new()
                            } else {
                                CStr::from_ptr(name).to_string_lossy().into_owned()
                            };
                            Some((name, TypeSpec::from_mms(child)))
                        })
                        .collect();
                    TypeSpec::Struct(fields)
                }
                ffi::MmsType_MMS_ARRAY => {
                    let n = ffi::MmsVariableSpecification_getSize(ptr).max(0) as usize;
                    let elem = ffi::MmsVariableSpecification_getArrayElementSpecification(ptr);
                    if elem.is_null() {
                        TypeSpec::Array(n, Box::new(TypeSpec::Leaf(ffi::MmsType_MMS_DATA_ACCESS_ERROR)))
                    } else {
                        TypeSpec::Array(n, Box::new(TypeSpec::from_mms(elem)))
                    }
                }
                other => TypeSpec::Leaf(other),
            }
        }
    }

    /// 按结构把 `value` 展开成叶子: `(相对路径, 值)`, 路径以 `.` 分隔(数组元素为 `[i]`),
    /// 本身是叶子时路径为空串。`Null` 和形状对不上的部分被跳过。
    pub fn leaves(&self, value: &Value) -> Vec<(String, Value)> {
        let mut out = Vec::new();
        self.walk(String::new(), value, &mut out);
        out
    }

    fn walk(&self, prefix: String, value: &Value, out: &mut Vec<(String, Value)>) {
        match (self, value) {
            (_, Value::Null) => {}
            (TypeSpec::Struct(fields), Value::Struct(values)) => {
                for ((name, spec), v) in fields.iter().zip(values) {
                    let path = if prefix.is_empty() {
                        name.clone()
                    } else {
                        format!("{prefix}.{name}")
                    };
                    spec.walk(path, v, out);
                }
            }
            (TypeSpec::Array(_, elem), Value::Array(values)) => {
                for (i, v) in values.iter().enumerate() {
                    elem.walk(format!("{prefix}[{i}]"), v, out);
                }
            }
            (TypeSpec::Leaf(_), v) => out.push((prefix, v.clone())),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_of_do_level_member() {
        // Ind1 的 ST 部分: {stVal, q, t}
        let spec = TypeSpec::Struct(vec![
            ("stVal".into(), TypeSpec::Leaf(2)),
            ("q".into(), TypeSpec::Leaf(3)),
            ("t".into(), TypeSpec::Leaf(14)),
        ]);
        let v = Value::Struct(vec![Value::Bool(true), Value::Quality(Quality::GOOD), Value::UtcTime(1)]);
        let leaves = spec.leaves(&v);
        assert_eq!(leaves.len(), 3);
        assert_eq!(leaves[0], ("stVal".to_string(), Value::Bool(true)));
        assert_eq!(leaves[2].0, "t");
    }

    #[test]
    fn leaves_of_nested_and_leaf_member() {
        let spec = TypeSpec::Struct(vec![(
            "cVal".into(),
            TypeSpec::Struct(vec![("mag".into(), TypeSpec::Struct(vec![("f".into(), TypeSpec::Leaf(6))]))]),
        )]);
        let v = Value::Struct(vec![Value::Struct(vec![Value::Struct(vec![Value::Float(1.5)])])]);
        assert_eq!(spec.leaves(&v), vec![("cVal.mag.f".to_string(), Value::Float(1.5))]);
        // 成员本身是叶子: 路径为空
        assert_eq!(TypeSpec::Leaf(6).leaves(&Value::Float(2.0)), vec![(String::new(), Value::Float(2.0))]);
    }

    #[test]
    fn skips_null_and_mismatched() {
        let spec = TypeSpec::Struct(vec![("a".into(), TypeSpec::Leaf(2)), ("b".into(), TypeSpec::Leaf(2))]);
        let v = Value::Struct(vec![Value::Null, Value::Bool(true)]);
        assert_eq!(spec.leaves(&v), vec![("b".to_string(), Value::Bool(true))]);
        assert!(spec.leaves(&Value::Null).is_empty());
        assert!(spec.leaves(&Value::Bool(true)).is_empty());
    }
}
