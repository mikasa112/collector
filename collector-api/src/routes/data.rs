use salvo::Router;

use crate::{handlers, middleware::auth::auth_handler};

/// 数据点位相关api
pub(crate) fn router() -> Router {
    Router::with_path("data")
        .hoop(auth_handler())
        .push(Router::with_path("set").post(handlers::data::set))
}
