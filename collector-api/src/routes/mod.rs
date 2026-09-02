mod data;
mod emu;
mod history;
#[cfg(target_os = "linux")]
mod network;
mod planned_curve;
mod user;
mod ws;

use crate::middleware::inject::InjectEg25;
use collector_core::utils::eg25::Eg25Info;
use salvo::Router;
use tokio::sync::watch;

pub(crate) fn root_router(eg25_rx: Option<watch::Receiver<Eg25Info>>) -> Router {
    let mut v1 = Router::new()
        .path("v1")
        .push(user::router())
        .push(data::router())
        .push(planned_curve::router())
        .push(history::router())
        .push(ws::router())
        .push(emu::router());
    if let Some(rx) = eg25_rx {
        v1 = v1.hoop(InjectEg25::new(rx));
    }
    #[cfg(target_os = "linux")]
    let v1 = v1.push(network::router());
    Router::new().push(v1)
}
