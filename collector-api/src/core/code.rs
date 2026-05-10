use salvo::{Depot, Request, Response, async_trait, http::StatusCode, writing::Json};

use crate::core::response::ObjResponse;
use crate::dao::error::DaoError;
use crate::services::error::ServiceError;
use collector_core::utils::database::DatabaseError;

/// 统一的错误码枚举
#[derive(Debug, thiserror::Error)]
pub enum Code {
    /// 自定义错误码和消息
    #[error("{1}: {0}")]
    #[allow(dead_code)]
    New(i32, String),

    /// 参数解析错误
    #[error("参数解析错误: {source}")]
    ParamsError {
        #[from]
        source: salvo::http::ParseError,
    },

    /// 参数校验错误
    #[error("参数校验错误: {validation_error}")]
    ValidationError {
        #[from]
        validation_error: validator::ValidationErrors,
    },

    /// Service 层错误（从 ServiceError 转换而来）
    #[error("服务错误: {0}")]
    ServiceError(#[from] ServiceError),

    /// DAO 层错误（保留用于直接使用）
    #[error("数据访问错误: {0}")]
    DaoError(#[from] DaoError),

    /// 数据库连接池错误（保留用于直接使用）
    #[error("数据库连接错误: {0}")]
    DatabaseError(#[from] DatabaseError),
}

impl Code {
    /// 获取 HTTP 状态码和错误信息
    fn to_status_and_message(&self) -> (StatusCode, i32, String) {
        match self {
            // 自定义错误
            Code::New(code, msg) => (StatusCode::BAD_REQUEST, *code, msg.clone()),

            // 参数错误
            Code::ParamsError { source } => (
                StatusCode::BAD_REQUEST,
                400,
                format!("参数解析错误: {}", source),
            ),

            // 参数校验错误
            Code::ValidationError { validation_error } => (
                StatusCode::BAD_REQUEST,
                400,
                format!("参数校验错误: {}", validation_error),
            ),

            // Service 层错误映射
            Code::ServiceError(service_err) => match service_err {
                ServiceError::NotFound(msg) => (StatusCode::NOT_FOUND, 404, msg.clone()),
                ServiceError::AlreadyExists(msg) => (StatusCode::CONFLICT, 409, msg.clone()),
                ServiceError::InvalidParameter(msg) => (StatusCode::BAD_REQUEST, 400, msg.clone()),
                ServiceError::BusinessLogic(msg) => (StatusCode::BAD_REQUEST, 400, msg.clone()),
                ServiceError::AuthenticationFailed(msg) => {
                    (StatusCode::UNAUTHORIZED, 401, msg.clone())
                }
                ServiceError::PermissionDenied(msg) => (StatusCode::FORBIDDEN, 403, msg.clone()),
                ServiceError::Dao(dao_err) => {
                    // 递归处理 DAO 错误
                    Self::map_dao_error(dao_err)
                }
                ServiceError::Database(db_err) => {
                    tracing::error!("数据库连接错误: {}", db_err);
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        503,
                        "服务暂时不可用".to_string(),
                    )
                }
                ServiceError::Join(_) => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    503,
                    "服务暂时不可用".to_string(),
                ),
            },

            // DAO 层错误映射（直接使用时）
            Code::DaoError(dao_err) => Self::map_dao_error(dao_err),

            // 数据库连接错误
            Code::DatabaseError(err) => {
                tracing::error!("数据库连接错误: {}", err);
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    503,
                    "服务暂时不可用".to_string(),
                )
            }
        }
    }

    /// 映射 DAO 错误到 HTTP 状态码
    fn map_dao_error(dao_err: &DaoError) -> (StatusCode, i32, String) {
        match dao_err {
            DaoError::NotFound(msg) => (StatusCode::NOT_FOUND, 404, msg.clone()),
            DaoError::AlreadyExists(msg) => (StatusCode::CONFLICT, 409, msg.clone()),
            DaoError::InvalidParameter(msg) => (StatusCode::BAD_REQUEST, 400, msg.clone()),
            DaoError::Database(err) => {
                // 数据库错误不暴露详细信息
                tracing::error!("数据库错误: {}", err);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    500,
                    "系统错误".to_string(),
                )
            }
            DaoError::DbPoolError(err) => {
                tracing::error!("数据库连接池错误: {}", err);
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    503,
                    "服务暂时不可用".to_string(),
                )
            }
            DaoError::OperationFailed(msg) => (StatusCode::INTERNAL_SERVER_ERROR, 500, msg.clone()),
        }
    }
}

#[async_trait]
impl salvo::Writer for Code {
    async fn write(self, _req: &mut Request, _depot: &mut Depot, res: &mut Response) {
        let (http_status, code, msg) = self.to_status_and_message();

        res.status_code(http_status);
        res.render(Json(ObjResponse::<()> {
            msg: Some(msg),
            status: code,
            data: None,
        }));
    }
}
