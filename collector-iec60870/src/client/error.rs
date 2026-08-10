#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    NullPtrErr(#[from] std::ffi::NulError),
    #[error("连接失败")]
    ConnectFailed,
    #[error("未连接")]
    NotConnected,
    #[error("发送{0}失败")]
    SendFailed(&'static str),
}
