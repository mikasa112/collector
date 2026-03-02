#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("frame too short")]
    FrameTooShort,
    #[error("bad magic: {0:#06x}")]
    BadMagic(u16),
    #[error("unsupported version: {0}")]
    UnsupportedVersion(u8),
    #[error("unsupported msg type: {0}")]
    UnsupportedMsgType(u8),
    #[error("header length out of range: {0}")]
    HeaderLengthOutOfRange(u16),
    #[error("frame length mismatch")]
    FrameLengthMismatch,
    #[error("device id length mismatch")]
    DeviceIdLengthMismatch,
    #[error("crc mismatch, expected={expected:#010x}, actual={actual:#010x}")]
    CrcMismatch { expected: u32, actual: u32 },
    #[error("field too large")]
    FieldTooLarge,
    #[error("compression error: {0}")]
    CompressionError(String),
}
