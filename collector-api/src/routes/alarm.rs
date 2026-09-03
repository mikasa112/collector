use salvo::Router;

use crate::handlers;

/// 告警历史查询api（无需鉴权）
pub(crate) fn router() -> Router {
    Router::with_path("alarm").push(Router::with_path("list").get(handlers::alarm::list))
}
