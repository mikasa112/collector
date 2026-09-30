use salvo::Router;

use crate::{handlers, middleware::auth::require_role, models::user::Role};

/// 字段绑定（元模型）管理接口
pub(crate) fn router() -> Router {
    Router::with_path("field_binding")
        .push(Router::with_path("list").get(handlers::field_binding::list))
        .push(
            Router::new()
                .hoop(require_role(Role::Admin))
                .put(handlers::field_binding::set_binding)
                .delete(handlers::field_binding::reset_binding),
        )
}
