mod user;
mod ws;

use crate::middleware::inject::InjectCenter;
use collector_core::center::SharedPointCenter;
use salvo::Router;

pub(crate) fn root_router(center: SharedPointCenter) -> Router {
    Router::new().push(
        Router::new()
            .hoop(InjectCenter::new(center))
            .path("v1")
            .push(user::router())
            .push(ws::router()),
    )
}
