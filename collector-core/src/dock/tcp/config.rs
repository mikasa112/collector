use std::time::Duration;

#[derive(Debug, Clone)]
pub struct TcpServerConfig {
    pub bind_addr: String,
    pub max_frame_len: usize,
    pub heartbeat_timeout: Duration,
    pub heartbeat_check_interval: Duration,
    pub push_interval: Duration,
    pub dict_ack_timeout: Duration,
    pub dict_ack_check_interval: Duration,
    pub dict_ack_max_retries: u32,
    pub enable_compression: bool,
    pub compress_threshold: usize,
    pub compress_level: i32,
}

impl Default for TcpServerConfig {
    fn default() -> Self {
        Self {
            bind_addr: "0.0.0.0:9000".to_string(),
            max_frame_len: 1024 * 1024,
            heartbeat_timeout: Duration::from_secs(30),
            heartbeat_check_interval: Duration::from_secs(5),
            push_interval: Duration::from_millis(1000),
            dict_ack_timeout: Duration::from_secs(3),
            dict_ack_check_interval: Duration::from_millis(500),
            dict_ack_max_retries: 3,
            enable_compression: true,
            compress_threshold: 256,
            compress_level: 1,
        }
    }
}
