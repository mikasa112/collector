use salvo::Router;

use crate::handlers;

/// 网络相关路由
pub(crate) fn router() -> Router {
    Router::with_path("network")
        .push(Router::with_path("wifi_scan").get(handlers::network::scan))
        .push(Router::with_path("wifi_connect").post(handlers::network::connect))
}
