use collector_core::{
    center::SharedPointCenter, shutdown::ShutdownManager, utils::eg25::Eg25Info,
};
use salvo::{Listener, Server, conn::TcpListener};
use tokio::sync::watch;
use tracing::info;

use crate::routes::root_router;

pub(crate) mod core;
pub(crate) mod dao;
pub(crate) mod handlers;
pub(crate) mod middleware;
pub(crate) mod models;
pub(crate) mod routes;
pub(crate) mod services;

pub struct ApiApp {
    ip: String,
    port: u16,
    center: SharedPointCenter,
    eg25_rx: Option<watch::Receiver<Eg25Info>>,
}

impl ApiApp {
    pub fn new(
        ip: String,
        port: u16,
        center: SharedPointCenter,
        eg25_rx: Option<watch::Receiver<Eg25Info>>,
    ) -> Self {
        Self {
            ip,
            port,
            center,
            eg25_rx,
        }
    }

    pub async fn start(self, shutdown: ShutdownManager) {
        let acceptor = TcpListener::new(format!("{}:{}", self.ip, self.port))
            .bind()
            .await;
        let server = Server::new(acceptor);
        let handle = server.handle();
        // 在后台任务中等待关闭信号
        let shutdown_handle = handle.clone();
        tokio::spawn(async move {
            shutdown.wait_for_shutdown().await;
            info!("API 服务器收到关闭信号，开始优雅关闭...");
            shutdown_handle.stop_graceful(None);
        });

        server.serve(root_router(self.center, self.eg25_rx)).await;
        info!("API 服务器已关闭");
    }
}
