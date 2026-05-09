use collector_core::utils::database::DatabaseError;
use thiserror::Error;

/// DAO 层错误类型
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum DaoError {
    #[error("数据库连接池错误: {0}")]
    DbPoolError(#[from] DatabaseError),
    /// 数据库错误
    #[error("数据库错误: {0}")]
    Database(#[from] sqlx::Error),

    /// 记录未找到
    #[error("记录未找到: {0}")]
    NotFound(String),

    /// 记录已存在
    #[error("记录已存在: {0}")]
    AlreadyExists(String),

    /// 无效的参数
    #[error("无效的参数: {0}")]
    InvalidParameter(String),

    /// 操作失败
    #[error("操作失败: {0}")]
    OperationFailed(String),
}

/// DAO 层结果类型
pub type DaoResult<T> = Result<T, DaoError>;
