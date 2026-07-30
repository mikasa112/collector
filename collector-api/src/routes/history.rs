use salvo::Router;

use crate::handlers;

/// 历史数据查询api（无需鉴权）
pub(crate) fn router() -> Router {
    Router::with_path("history")
        .push(Router::with_path("pcs").get(handlers::history::pcs_history))
        .push(Router::with_path("bcu").get(handlers::history::bcu_history))
}
