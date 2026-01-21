use std::fmt::Debug;

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub enum Val {
    I16(i16),
    I32(i32),
    U16(u16),
    U32(u32),
    F32(f32),
    Str(&'static str),
}

pub trait Point: Copy + Clone + Send + Sync + Debug {
    fn key(&self) -> &'static str;
    fn value(&self) -> Val;
}
