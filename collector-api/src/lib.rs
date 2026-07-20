use collector_core::{
    center::SharedPointCenter,
    shutdown::ShutdownManager,
    utils::database::close_database,
};
use salvo::{Listener, Server, conn::TcpListener};
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
}

impl ApiApp {
    pub fn new(ip: String, port: u16, center: SharedPointCenter) -> Self {
        Self { ip, port, center }
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
            close_database().await;
        });

        server.serve(root_router(self.center)).await;
        info!("API 服务器已关闭");
    }
}
