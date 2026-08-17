use salvo::Router;

use crate::handlers;

pub(crate) fn router() -> Router {
    Router::with_path("ws")
        .push(Router::with_path("data").goal(handlers::ws::data_ws_handler))
        .push(Router::with_path("home").goal(handlers::ws::home_ws_handler))
        .push(Router::with_path("history").goal(handlers::ws::history_ws_handler))
        .push(Router::with_path("eg25").goal(handlers::ws::eg25_ws_handler))
}
