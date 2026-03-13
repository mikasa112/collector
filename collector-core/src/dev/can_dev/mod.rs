mod backoff;
pub mod device;
mod downlink;
mod runner;

pub use device::CanDev;

#[derive(Debug, thiserror::Error)]
pub enum CanDevError {
    #[error("打开CAN接口失败: {0}")]
    OpenSocket(#[from] std::io::Error),
    #[error("CAN读帧失败: {0}")]
    ReadFrame(std::io::Error),
    #[error("CAN写帧失败: {0}")]
    WriteFrame(std::io::Error),
    #[error("CAN总线超时: {0}")]
    Timeout(String),
}
