use salvo::Router;

use crate::{handlers, middleware::auth::require_role, models::user::Role};

/// 数据点位相关api
pub(crate) fn router() -> Router {
    Router::new()
        .push(
            Router::with_path("data")
                .hoop(require_role(Role::Admin))
                .push(Router::with_path("set").post(handlers::data::set)),
        )
        .push(Router::with_path("devices").get(handlers::data::list_devices))
}
