use collector_core::{
    shutdown::ShutdownManager,
    utils::database::{DatabaseConfig, close_database, init_database},
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
}

impl Default for ApiApp {
    fn default() -> Self {
        Self {
            ip: String::from("0.0.0.0"),
            port: 9091,
        }
    }
}

impl ApiApp {
    pub fn new(ip: String, port: u16) -> Self {
        Self { ip, port }
    }

    pub async fn start(self, shutdown: ShutdownManager) {
        let acceptor = TcpListener::new(format!("{}:{}", self.ip, self.port))
            .bind()
            .await;
        let server = Server::new(acceptor);
        let handle = server.handle();
        if let Err(e) = init_database(DatabaseConfig::default()).await {
            tracing::error!("{}", e)
        }
        // 在后台任务中等待关闭信号
        let shutdown_handle = handle.clone();
        tokio::spawn(async move {
            shutdown.wait_for_shutdown().await;
            info!("API 服务器收到关闭信号，开始优雅关闭...");
            shutdown_handle.stop_graceful(None);
            close_database().await;
        });

        server.serve(root_router()).await;
        info!("API 服务器已关闭");
    }
}
