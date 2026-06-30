mod data;
#[cfg(target_os = "linux")]
mod network;
mod user;
mod ws;

use crate::middleware::inject::InjectCenter;
use collector_core::center::SharedPointCenter;
use salvo::Router;

pub(crate) fn root_router(center: SharedPointCenter) -> Router {
    let v1 = Router::new()
        .hoop(InjectCenter::new(center))
        .path("v1")
        .push(user::router())
        .push(data::router())
        .push(ws::router());
    #[cfg(target_os = "linux")]
    let v1 = v1.push(network::router());
    Router::new().push(v1)
}
