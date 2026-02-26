use std::fmt::Debug;

use serde::Serialize;

pub type PointId = u64;
pub type PointKey = &'static str;

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
