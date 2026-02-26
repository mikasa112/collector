use std::fmt::Debug;

use serde::Serialize;

pub type PointId = u64;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Val {
    U8(u8),
    I8(i8),
    I16(i16),
    I32(i32),
    U16(u16),
    U32(u32),
    F32(f32),
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
        }
    }
}

pub trait Point: Send + Sync + Copy + Clone {
    fn id(&self) -> u64;
    fn name(&self) -> &'static str;
    fn value(&self) -> Val;
}

#[derive(Debug, Serialize, Clone, Copy)]
pub struct DataPoint {
    pub id: u64,
    pub name: &'static str,
    pub value: Val,
}

impl Point for DataPoint {
    fn id(&self) -> u64 {
        self.id
    }

    fn name(&self) -> &'static str {
        self.name
    }

    fn value(&self) -> Val {
        self.value
    }
}
