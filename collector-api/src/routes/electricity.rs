use salvo::Router;

use crate::{handlers, middleware::auth::require_role, models::user::Role};

/// 电价配置接口：读取与其他配置类接口一致不要求登录，修改要求 Admin
pub(crate) fn router() -> Router {
    Router::with_path("electricity")
        .push(
            Router::with_path("price")
                .get(handlers::electricity::list_prices)
                .push(
                    Router::new()
                        .hoop(require_role(Role::Admin))
                        .put(handlers::electricity::update_prices),
                ),
        )
        .push(
            Router::with_path("period")
                .get(handlers::electricity::list_periods)
                .push(
                    Router::new()
                        .hoop(require_role(Role::Admin))
                        .put(handlers::electricity::replace_periods),
                ),
        )
}
