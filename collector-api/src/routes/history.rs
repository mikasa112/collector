use salvo::Router;

use crate::{handlers, middleware::auth::require_role, models::user::Role};

/// 历史数据查询api
pub(crate) fn router() -> Router {
    Router::with_path("history")
        .hoop(require_role(Role::Admin))
        .push(Router::with_path("pcs").get(handlers::history::pcs_history))
        .push(Router::with_path("bcu").get(handlers::history::bcu_history))
}
