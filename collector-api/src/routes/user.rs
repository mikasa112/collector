use crate::{
    handlers,
    middleware::auth::require_role,
    models::user::Role,
};
use salvo::Router;

/// 用户相关路由：用户管理整组要求 Admin 及以上，越权细则在 service 层校验
pub(crate) fn router() -> Router {
    Router::new()
        .push(Router::with_path("login").post(handlers::user::login))
        .push(
            Router::with_path("user")
                .hoop(require_role(Role::Admin))
                .post(handlers::user::create_user)
                .push(Router::with_path("list").get(handlers::user::list_users))
                .push(
                    Router::with_path("{id}")
                        .put(handlers::user::update_user)
                        .delete(handlers::user::delete_user),
                ),
        )
}
