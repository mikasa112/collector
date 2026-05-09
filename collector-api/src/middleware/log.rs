use salvo::http::{ResBody, StatusCode};
use salvo::{Depot, FlowCtrl, Request, Response, async_trait};
use std::time::Instant;
use tracing::info;

#[derive(Default)]
pub struct LogMiddleware;

impl LogMiddleware {
    #[inline]
    pub fn new() -> Self {
        LogMiddleware {}
    }
}

#[async_trait]
impl salvo::Handler for LogMiddleware {
    async fn handle(
        &self,
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        ctrl: &mut FlowCtrl,
    ) {
        let addr = req.remote_addr().to_string();
        let method = req.method().as_str().to_string();
        let path = req.uri().path().to_string();
        async move {
            let now = Instant::now();
            ctrl.call_next(req, depot, res).await;
            let duration = now.elapsed();
            let status = res.status_code.unwrap_or(match &res.body {
                ResBody::None => StatusCode::NOT_FOUND,
                ResBody::Error(e) => e.code,
                _ => StatusCode::OK,
            });
            info!("Request({addr} {method} {path} {status:?} {duration:?})");
        }
        .await;
    }
}
