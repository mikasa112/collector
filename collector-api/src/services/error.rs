use crate::dao::error::DaoError;
use collector_core::utils::database::DatabaseError;
use thiserror::Error;

/// Service 层错误类型（纯业务错误，不包含 HTTP 概念）
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum ServiceError {
    /// DAO 层错误
    #[error("数据访问错误: {0}")]
    Dao(#[from] DaoError),

    /// 数据库连接错误
    #[error("数据库连接错误: {0}")]
    Database(#[from] DatabaseError),

    /// 业务逻辑错误
    #[error("{0}")]
    BusinessLogic(String),

    /// 认证失败
    #[error("{0}")]
    AuthenticationFailed(String),

    /// 权限不足
    #[error("{0}")]
    PermissionDenied(String),

    /// 资源不存在
    #[error("{0}")]
    NotFound(String),

    /// 资源已存在
    #[error("{0}")]
    AlreadyExists(String),

    /// 无效的参数
    #[error("{0}")]
    InvalidParameter(String),

    /// 线程池错误
    #[error("{0}")]
    Join(#[from] tokio::task::JoinError),

    /// 系统内部错误
    #[error("{0}")]
    InternalError(String),
}

/// Service 层结果类型
pub type ServiceResult<T> = Result<T, ServiceError>;

#[allow(dead_code)]
impl ServiceError {
    /// 创建业务逻辑错误
    pub fn business_logic(msg: impl Into<String>) -> Self {
        Self::BusinessLogic(msg.into())
    }

    /// 创建认证失败错误
    pub fn auth_failed(msg: impl Into<String>) -> Self {
        Self::AuthenticationFailed(msg.into())
    }

    /// 创建权限不足错误
    pub fn permission_denied(msg: impl Into<String>) -> Self {
        Self::PermissionDenied(msg.into())
    }

    /// 创建资源不存在错误
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    /// 创建资源已存在错误
    pub fn already_exists(msg: impl Into<String>) -> Self {
        Self::AlreadyExists(msg.into())
    }

    /// 创建无效参数错误
    pub fn invalid_parameter(msg: impl Into<String>) -> Self {
        Self::InvalidParameter(msg.into())
    }
}
