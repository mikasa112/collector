use std::fmt::Debug;

use serde::Serialize;

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

pub trait Point: Send + Sync + Clone {
    fn key(&self) -> String;
    fn value(&self) -> Val;
}
