use salvo::Router;

use crate::{handlers, middleware::log::LogMiddleware};

pub(crate) fn open_router() -> Router {
    Router::new()
        .push(Router::with_path("login").post(handlers::user::login))
        .push(Router::with_path("user").post(handlers::user::create_user))
}

pub(crate) fn root_router() -> Router {
    Router::new().push(
        Router::new()
            .hoop(LogMiddleware::new())
            .path("v1")
            .push(open_router()),
    )
}
