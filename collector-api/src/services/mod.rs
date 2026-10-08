pub mod alarm;
pub mod arbitrage;
pub mod data;
pub mod electricity;
#[cfg(target_os = "linux")]
pub mod ethernet;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub mod ethernet_cfg;
pub mod emu;
pub mod error;
pub mod field_binding;
pub mod history;
#[cfg(target_os = "linux")]
pub mod network;
pub mod planned_curve;
pub mod project_info;
#[cfg(target_os = "linux")]
pub mod script;
#[cfg(target_os = "linux")]
pub mod system;
pub mod user;

// Service 层使用独立的错误类型
pub use error::{ServiceError, ServiceResult};
