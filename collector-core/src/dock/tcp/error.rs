use crate::dock::frame::FrameError;

#[derive(Debug, thiserror::Error)]
pub enum TcpServerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame error: {0}")]
    FrameError(#[from] FrameError),
    #[error("frame too large: {0}")]
    FrameTooLarge(usize),
    #[error("header error: {0}")]
    HeaderError(String),
}
