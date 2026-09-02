use collector_core::utils::eg25::Eg25Info;
use salvo::{Depot, FlowCtrl, Handler, Request, Response, async_trait};
use tokio::sync::watch;

#[derive(Clone)]
pub struct InjectEg25 {
    rx: watch::Receiver<Eg25Info>,
}

impl InjectEg25 {
    pub fn new(rx: watch::Receiver<Eg25Info>) -> Self {
        Self { rx }
    }
}

#[async_trait]
impl Handler for InjectEg25 {
    async fn handle(
        &self,
        _req: &mut Request,
        depot: &mut Depot,
        _res: &mut Response,
        _ctrl: &mut FlowCtrl,
    ) {
        depot.insert("eg25", self.rx.clone());
    }
}
