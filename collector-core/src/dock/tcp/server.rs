use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::watch;

use super::config::TcpServerConfig;
use super::error::TcpServerError;
use super::session::handle_connection;
use super::traits::{FrameHandler, LoggingFrameHandler, NoopPushFrameProvider, PushFrameProvider};

pub struct TcpFrameServer {
    cfg: TcpServerConfig,
    handler: Arc<dyn FrameHandler>,
    push_provider: Arc<dyn PushFrameProvider>,
}

impl TcpFrameServer {
    pub fn new(cfg: TcpServerConfig) -> Self {
        Self {
            cfg,
            handler: Arc::new(LoggingFrameHandler),
            push_provider: Arc::new(NoopPushFrameProvider),
        }
    }

    pub fn with_handler(mut self, handler: Arc<dyn FrameHandler>) -> Self {
        self.handler = handler;
        self
    }

    pub fn with_push_provider(mut self, provider: Arc<dyn PushFrameProvider>) -> Self {
        self.push_provider = provider;
        self
    }

    pub async fn run(self) -> Result<(), TcpServerError> {
        let listener = TcpListener::bind(self.cfg.bind_addr.as_str()).await?;
        tracing::info!("TCP frame server listening on {}", self.cfg.bind_addr);
        loop {
            let (socket, peer) = listener.accept().await?;
            let handler = Arc::clone(&self.handler);
            let push_provider = Arc::clone(&self.push_provider);
            let cfg = self.cfg.clone();
            tokio::spawn(async move {
                if let Err(err) = handle_connection(socket, peer, cfg, handler, push_provider).await
                {
                    tracing::warn!("connection {} closed: {}", peer, err);
                }
            });
        }
    }

    pub async fn run_until_shutdown(
        self,
        mut shutdown_rx: watch::Receiver<bool>,
    ) -> Result<(), TcpServerError> {
        let listener = TcpListener::bind(self.cfg.bind_addr.as_str()).await?;
        tracing::info!("TCP frame server listening on {}", self.cfg.bind_addr);
        loop {
            tokio::select! {
                changed = shutdown_rx.changed() => {
                    if changed.is_ok() && *shutdown_rx.borrow() {
                        tracing::info!("TCP frame server stopped");
                        return Ok(());
                    }
                }
                accepted = listener.accept() => {
                    let (socket, peer) = accepted?;
                    let handler = Arc::clone(&self.handler);
                    let push_provider = Arc::clone(&self.push_provider);
                    let cfg = self.cfg.clone();
                    tokio::spawn(async move {
                        if let Err(err) = handle_connection(socket, peer, cfg, handler, push_provider).await {
                            tracing::warn!("connection {} closed: {}", peer, err);
                        }
                    });
                }
            }
        }
    }
}
