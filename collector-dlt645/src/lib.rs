//! DL/T 645-2007 主站协议实现:编解码与客户端只依赖 `AsyncRead + AsyncWrite`,
//! 串口的打开、轮询调度与重连由上层(collector-core 的 dlt645_dev)负责。

pub mod address;
pub mod client;
pub mod codec;
pub mod frame;
pub mod point;
pub mod spec;

pub use address::MeterAddress;
pub use client::{Client, ClientConfig, MeterError, Request};
pub use codec::Dl645Codec;
pub use frame::{DataIdentifier, Frame, FrameError};
pub use point::{PointValue, ValueError, decode_bcd_le, decode_signed_bcd_le, decode_unsigned_le};
pub use spec::{DiSpec, spec_for};
