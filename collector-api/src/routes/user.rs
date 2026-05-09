use crate::handlers;
use salvo::Router;

/// 用户相关路由
pub(crate) fn router() -> Router {
    Router::new()
        .push(Router::with_path("login").post(handlers::user::login))
        .push(Router::with_path("user").post(handlers::user::create_user))
}
