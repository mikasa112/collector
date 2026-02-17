use std::fmt::Debug;

use serde::Serialize;

pub type PointId = u64;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub enum Val {
    U8(u8),
    I8(i8),
    I16(i16),
    I32(i32),
    U16(u16),
    U32(u32),
    F32(f32),
}

pub trait Point: Send + Sync + Copy + Clone {
    fn key(&self) -> PointId;
    fn value(&self) -> Val;
}

#[derive(Debug, Serialize, Clone, Copy)]
pub struct DataPoint {
    pub key: PointId,
    pub value: Val,
}

impl Point for DataPoint {
    fn key(&self) -> u64 {
        self.key
    }

    fn value(&self) -> Val {
        self.value
    }
}

pub trait Item: Send + Sync {
    fn id(&self) -> PointId;
    fn name(&self) -> &str;
    fn unit(&self) -> Option<&str>;
}

pub struct Record {
    pub id: PointId,
    pub name: String,
    pub unit: Option<String>,
}

impl Item for Record {
    fn id(&self) -> PointId {
        self.id
    }

    fn name(&self) -> &str {
        self.name.as_str()
    }

    fn unit(&self) -> Option<&str> {
        self.unit.as_deref()
    }
}
