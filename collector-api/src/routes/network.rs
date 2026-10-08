use salvo::Router;

use crate::{handlers, middleware::auth::require_role, models::user::Role};

/// 网络相关路由
pub(crate) fn router() -> Router {
    Router::with_path("network")
        .push(Router::with_path("wifi_scan").get(handlers::network::scan))
        .push(Router::with_path("wifi_connect").post(handlers::network::connect))
        .push(
            Router::with_path("ethernet")
                .get(handlers::network::ethernet_list)
                .push(
                    Router::new()
                        .hoop(require_role(Role::Admin))
                        .post(handlers::network::ethernet_set),
                )
                .push(
                    Router::with_path("confirm")
                        .hoop(require_role(Role::Admin))
                        .post(handlers::network::ethernet_confirm),
                ),
        )
}
