pub mod error;
#[cfg(target_os = "linux")]
pub mod network;
pub mod user;

// Service 层使用独立的错误类型
pub use error::{ServiceError, ServiceResult};
