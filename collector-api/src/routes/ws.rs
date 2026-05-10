use salvo::Router;

use crate::handlers;

pub(crate) fn router() -> Router {
    Router::with_path("ws").goal(handlers::ws::ws_handler)
}
