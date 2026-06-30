use salvo::Router;

use crate::handlers;

/// 数据点位相关api
pub(crate) fn router() -> Router {
    Router::with_path("data").push(Router::with_path("set").post(handlers::data::set))
}
