use collector_core::center::SharedPointCenter;
use salvo::{Depot, FlowCtrl, Handler, Request, Response, async_trait};

#[derive(Clone)]
pub struct InjectCenter {
    center: SharedPointCenter,
}

impl InjectCenter {
    pub fn new(center: SharedPointCenter) -> Self {
        Self { center }
    }
}

#[async_trait]
impl Handler for InjectCenter {
    async fn handle(
        &self,
        _req: &mut Request,
        depot: &mut Depot,
        _res: &mut Response,
        _ctrl: &mut FlowCtrl,
    ) {
        depot.insert("center", self.center.clone());
    }
}
