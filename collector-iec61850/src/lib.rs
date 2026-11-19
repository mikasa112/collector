// libiec61850 的封装仅支持 Linux: build.rs 在其他平台不编译 C 源码, 这里同步让整个 crate 为空。
#![cfg(target_os = "linux")]

mod client;
// 启用 `server` feature 时公开原始绑定, 供测试用 IED(examples/tests)搭建服务端模型。
#[cfg(feature = "server")]
#[doc(hidden)]
pub mod ffi;
#[cfg(not(feature = "server"))]
mod ffi;

/// 测试用的模拟 PCS, 见模块文档。
#[cfg(feature = "server")]
#[doc(hidden)]
pub mod testing;

pub use client::{
    ConnectOptions, Connection, ConnectionState, ControlFailure, ControlOptions, DataSetLayout,
    Dbpos, Error, Fc, IsoParams, ObjectRef, Quality, REASON_DATA_CHANGE, REASON_DATA_UPDATE,
    REASON_GI, REASON_INTEGRITY, REASON_QUALITY_CHANGE, Report, ReportConfig, ReportEntry,
    TRG_DATA_CHANGED, TRG_DATA_UPDATE, TRG_GI, TRG_INTEGRITY, TRG_QUALITY_CHANGED, TypeSpec, Value,
};
