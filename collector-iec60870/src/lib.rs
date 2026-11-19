// lib60870-C 的封装仅支持 Linux: build.rs 在其他平台不编译 C 源码, 这里同步让整个 crate 为空。
#![cfg(target_os = "linux")]

mod client;
mod ffi;
mod server;

pub use client::{
    ApciParameters, Asdu, Client, ConnectionEvent, DoublePointValue, Error, InformationObjectData,
    InformationObjectValue, StepCommandValue,
};
pub use server::{
    CommandHandler, CommandValue, PointValue, ReceivedCommand, Slave, SlaveOptions, SlavePoint,
    quality,
};
