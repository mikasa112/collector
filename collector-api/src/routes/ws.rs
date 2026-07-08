use salvo::Router;

use crate::handlers;

pub(crate) fn router() -> Router {
    Router::with_path("ws")
        .push(Router::with_path("data").goal(handlers::ws::data_ws_handler))
        .push(Router::with_path("home").goal(handlers::ws::home_ws_handler))
}
