use std::fmt::{Debug, Display};

use serde::{Serialize, ser::SerializeSeq};

#[derive(Debug, thiserror::Error)]
pub enum ValError {
    #[error("Invalid value")]
    InvalidValue,
}

pub type PointId = u32;

#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    U8(u8),
    I8(i8),
    I16(i16),
    I32(i32),
    U16(u16),
    U32(u32),
    F32(f32),
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
        Val::F32(v) => Ok(v.abs() > f32::EPSILON),
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
        Val::F32(v) => Ok(*v as f64),
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
            Val::F32(v) => serializer.serialize_f32(*v),
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
            Val::F32(v) => write!(f, "{}", *v),
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

pub trait Point: Send + Sync + Clone {
    fn id(&self) -> PointId;
    fn name(&self) -> &'static str;
    fn value(&self) -> &Val;
}

#[derive(Debug, Serialize, Clone)]
pub struct DataPoint {
    pub id: PointId,
    pub name: &'static str,
    pub value: Val,
}

impl Point for DataPoint {
    fn id(&self) -> PointId {
        self.id
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn value(&self) -> &Val {
        &self.value
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
