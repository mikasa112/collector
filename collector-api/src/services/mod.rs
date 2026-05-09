pub mod error;
pub mod user;

// Service 层使用独立的错误类型
pub use error::{ServiceError, ServiceResult};
