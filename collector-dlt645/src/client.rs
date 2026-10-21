use crate::{
    DiSpec, Dl645Codec, MeterAddress,
    frame::{CTRL_EXCEPTION_BIT, CTRL_READ_DATA, CTRL_READ_DATA_RESPONSE, DataIdentifier, Frame},
};
use futures::{SinkExt, StreamExt};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::{Duration, timeout},
};
use tokio_util::codec::Framed;

#[derive(Debug, Clone)]
pub struct ClientConfig {
    pub response_timeout: Duration,
    pub wakeup_bytes: usize,
}
impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            response_timeout: Duration::from_millis(1200),
            wakeup_bytes: 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub address: MeterAddress,
    pub identifier: DataIdentifier,
}

#[derive(Debug, Error)]
pub enum MeterError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("protocol frame error: {0}")]
    Frame(#[from] crate::frame::FrameError),
    #[error("response timed out")]
    Timeout,
    #[error("response address did not match request")]
    AddressMismatch,
    #[error("unexpected control code 0x{0:02X}")]
    UnexpectedControl(u8),
    #[error("meter returned exception code 0x{0:02X}")]
    MeterException(u8),
    #[error("response data identifier did not match request")]
    IdentifierMismatch,
    #[error("response data is shorter than the 4-byte identifier")]
    MissingIdentifier,
    #[error("invalid value: {0}")]
    Value(#[from] crate::point::ValueError),
}

pub struct Client<S> {
    framed: Framed<S, Dl645Codec>,
    config: ClientConfig,
}

impl<S> Client<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(io: S, config: ClientConfig) -> Self {
        let framed = Framed::new(io, Dl645Codec::new(config.wakeup_bytes));
        Self { framed, config }
    }

    pub fn into_inner(self) -> S {
        self.framed.into_inner()
    }

    pub async fn read_data(
        &mut self,
        address: MeterAddress,
        identifier: DataIdentifier,
    ) -> Result<Vec<u8>, MeterError> {
        let request = Frame {
            address,
            control: CTRL_READ_DATA,
            data: identifier.to_wire_data().to_vec(),
        };
        self.discard_stale().await;
        let frame = timeout(self.config.response_timeout, self.exchange(request))
            .await
            .map_err(|_| MeterError::Timeout)??;
        if frame.address != address {
            return Err(MeterError::AddressMismatch);
        }
        if frame.control & CTRL_EXCEPTION_BIT != 0 {
            return Err(MeterError::MeterException(
                frame.data.first().copied().unwrap_or(0),
            ));
        }
        if frame.control != CTRL_READ_DATA_RESPONSE {
            return Err(MeterError::UnexpectedControl(frame.control));
        }
        if frame.data.len() < 4 {
            return Err(MeterError::MissingIdentifier);
        }
        if DataIdentifier::from_wire_data(&frame.data) != Some(identifier) {
            return Err(MeterError::IdentifierMismatch);
        }
        Ok(frame.data[4..].to_vec())
    }

    /// 读取并按 `spec` 解码为工程值(已应用小数位和符号)。
    pub async fn read_value(
        &mut self,
        address: MeterAddress,
        spec: &DiSpec,
    ) -> Result<f64, MeterError> {
        let data = self.read_data(address, spec.di).await?;
        Ok(spec.decode(&data)?)
    }

    /// 发送请求并等待一帧完整响应。`Framed` 的收发缓冲跨调用保留,被 `timeout` 取消也不会丢字节。
    async fn exchange(&mut self, request: Frame) -> Result<Frame, MeterError> {
        self.framed.send(request).await?;
        match self.framed.next().await {
            Some(frame) => frame,
            None => Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into()),
        }
    }

    /// 发送前丢弃上一次超时后迟到的响应:先清空已缓冲的数据,再把串口里已到达的字节读空。
    async fn discard_stale(&mut self) {
        self.framed.write_buffer_mut().clear();
        self.framed.read_buffer_mut().clear();
        // 零时长 timeout 会先 poll 一次内部 future:有数据则取走,没有(Pending)立即返回
        while let Ok(Some(_)) = timeout(Duration::ZERO, self.framed.next()).await {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt, duplex};

    const DI: DataIdentifier = DataIdentifier::from_u32(0x00010000);

    fn response(addr: &MeterAddress, value: [u8; 4]) -> Vec<u8> {
        let mut data = DI.to_wire_data().to_vec();
        data.extend_from_slice(&value);
        Frame {
            address: *addr,
            control: CTRL_READ_DATA_RESPONSE,
            data,
        }
        .encode()
        .unwrap()
    }

    fn config(ms: u64) -> ClientConfig {
        ClientConfig {
            response_timeout: Duration::from_millis(ms),
            wakeup_bytes: 4,
        }
    }

    #[tokio::test]
    async fn reads_data_with_fe_prefix_and_fragmented_response() {
        let addr = MeterAddress::parse("123456789012").unwrap();
        let (io, mut meter) = duplex(256);
        let resp = response(&addr, [0x78, 0x56, 0x34, 0x12]);
        let task = tokio::spawn(async move {
            let mut req = [0u8; 20];
            meter.read_exact(&mut req).await.unwrap();
            meter.write_all(&[0xFE, 0xFE]).await.unwrap();
            meter.write_all(&resp[..5]).await.unwrap();
            tokio::task::yield_now().await;
            meter.write_all(&resp[5..]).await.unwrap();
        });
        let mut client = Client::new(io, config(500));
        let value = client.read_data(addr, DI).await.unwrap();
        task.await.unwrap();
        assert_eq!(value, vec![0x78, 0x56, 0x34, 0x12]);
    }

    #[tokio::test]
    async fn late_response_after_timeout_does_not_pollute_next_request() {
        let addr = MeterAddress::parse("1").unwrap();
        let (io, mut meter) = duplex(256);
        let mut client = Client::new(io, config(50));

        // 第一次:电表不回 -> 超时
        assert!(matches!(
            client.read_data(addr, DI).await,
            Err(MeterError::Timeout)
        ));

        // 迟到的响应(含半帧)此时才到达串口,随后第二次请求发出
        let stale = response(&addr, [1, 0, 0, 0]);
        meter.write_all(&stale).await.unwrap();
        meter.write_all(&stale[..6]).await.unwrap();

        let fresh = response(&addr, [9, 0, 0, 0]);
        let task = tokio::spawn(async move {
            let mut req = [0u8; 20];
            // 第一次请求的残留 + 第二次请求
            meter.read_exact(&mut req).await.unwrap();
            meter.read_exact(&mut req).await.unwrap();
            meter.write_all(&fresh).await.unwrap();
        });
        let value = client.read_data(addr, DI).await.unwrap();
        task.await.unwrap();
        assert_eq!(value, vec![9, 0, 0, 0]);
    }
}
