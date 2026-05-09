use collector_core::shutdown::ShutdownManager;
use salvo::{Listener, Router, Server, conn::TcpListener};
use tracing::info;

pub struct ApiServer {
    ip: String,
    port: u16,
}

impl Default for ApiServer {
    fn default() -> Self {
        Self {
            ip: String::from("0.0.0.0"),
            port: 9091,
        }
    }
}

impl ApiServer {
    pub fn new(ip: String, port: u16) -> Self {
        Self { ip, port }
    }

    pub async fn start(&self, shutdown: ShutdownManager) {
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

        server.serve(Router::new()).await;
        info!("API 服务器已关闭");
    }
}
