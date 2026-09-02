pub mod data;
pub mod emu;
pub mod error;
pub mod history;
#[cfg(target_os = "linux")]
pub mod network;
pub mod planned_curve;
pub mod user;

// Service 层使用独立的错误类型
pub use error::{ServiceError, ServiceResult};
