use crate::{
    client::MeterError,
    frame::{Frame, MAX_DATA_LEN, START},
};
use bytes::{Buf, BufMut, BytesMut};
use tokio_util::codec::{Decoder, Encoder};

/// 帧头(10) + CS(1) + 结束符(1)
const MIN_FRAME_LEN: usize = 12;
/// 第二个 0x68 在帧内的偏移
const SECOND_START_INDEX: usize = 7;
/// 数据域长度字节在帧内的偏移
const LEN_INDEX: usize = 9;

/// DL/T 645 帧编解码器。
///
/// 解码侧自带重新同步:任何一段候选帧校验失败(第二个起始符、长度、结束符、校验和),
/// 只丢弃其首字节(0x68)后继续向后搜索,因此前导 0xFE、总线噪声、上一次超时遗留的半帧
/// 都不会污染后续帧。缓冲由 `BytesMut` 持有,`Framed::next()` 被 `timeout` 取消后不会丢字节。
#[derive(Debug, Clone, Default)]
pub struct Dl645Codec {
    /// 发送时附加的 0xFE 前导字节个数
    wakeup_bytes: usize,
}

impl Dl645Codec {
    pub fn new(wakeup_bytes: usize) -> Self {
        Self { wakeup_bytes }
    }
}

impl Decoder for Dl645Codec {
    type Item = Frame;
    type Error = MeterError;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Frame>, MeterError> {
        loop {
            // 丢弃第一个 0x68 之前的所有字节(FE 前导、噪声)
            match src.iter().position(|&b| b == START) {
                Some(pos) => src.advance(pos),
                None => {
                    src.clear();
                    return Ok(None);
                }
            }
            // 第二个起始符一旦可见就先校验,避免噪声里的 0x68 把解码器卡在等待长数据上
            if src.len() > SECOND_START_INDEX && src[SECOND_START_INDEX] != START {
                src.advance(1);
                continue;
            }
            if src.len() <= LEN_INDEX {
                return Ok(None);
            }
            let data_len = src[LEN_INDEX] as usize;
            if data_len > MAX_DATA_LEN {
                src.advance(1);
                continue;
            }
            let total = MIN_FRAME_LEN + data_len;
            if src.len() < total {
                src.reserve(total - src.len());
                return Ok(None);
            }
            match Frame::decode(&src[..total]) {
                Ok(frame) => {
                    src.advance(total);
                    return Ok(Some(frame));
                }
                // 结束符/校验和不对:这个 0x68 不是真正的帧头,继续找下一个
                Err(_) => src.advance(1),
            }
        }
    }
}

impl Encoder<Frame> for Dl645Codec {
    type Error = MeterError;

    fn encode(&mut self, frame: Frame, dst: &mut BytesMut) -> Result<(), MeterError> {
        let raw = frame.encode()?;
        dst.reserve(self.wakeup_bytes + raw.len());
        dst.put_bytes(0xFE, self.wakeup_bytes);
        dst.extend_from_slice(&raw);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MeterAddress;

    fn sample() -> (Frame, Vec<u8>) {
        let frame = Frame {
            address: MeterAddress::parse("1").unwrap(),
            control: 0x91,
            data: vec![0, 1, 2, 3, 0x12, 0x34],
        };
        let raw = frame.encode().unwrap();
        (frame, raw)
    }

    #[test]
    fn skips_fe_prefix() {
        let (frame, raw) = sample();
        let mut buf = BytesMut::from(&[0xFE, 0xFE, 0xFE][..]);
        buf.extend_from_slice(&raw);
        let got = Dl645Codec::default().decode(&mut buf).unwrap();
        assert_eq!(got, Some(frame));
        assert!(buf.is_empty());
    }

    #[test]
    fn fragmented_input_is_buffered() {
        let (frame, raw) = sample();
        let mut codec = Dl645Codec::default();
        let mut buf = BytesMut::new();
        for chunk in raw.chunks(3) {
            assert_eq!(codec.decode(&mut buf).unwrap(), None);
            buf.extend_from_slice(chunk);
        }
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(frame));
    }

    #[test]
    fn resyncs_after_noise_containing_start_byte() {
        let (frame, raw) = sample();
        // 噪声里夹着 0x68,后面紧跟真实帧
        let mut buf = BytesMut::from(&[0x01, 0x68, 0x02, 0x03][..]);
        buf.extend_from_slice(&raw);
        let got = Dl645Codec::default().decode(&mut buf).unwrap();
        assert_eq!(got, Some(frame));
    }

    #[test]
    fn resyncs_after_corrupted_frame() {
        let (frame, raw) = sample();
        let mut bad = raw.clone();
        bad[11] ^= 0xFF; // 破坏数据 -> 校验和错误
        let mut buf = BytesMut::from(&bad[..]);
        buf.extend_from_slice(&raw);
        let got = Dl645Codec::default().decode(&mut buf).unwrap();
        assert_eq!(got, Some(frame));
    }

    #[test]
    fn decodes_back_to_back_frames() {
        let (frame, raw) = sample();
        let mut buf = BytesMut::from(&raw[..]);
        buf.extend_from_slice(&raw);
        let mut codec = Dl645Codec::default();
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(frame.clone()));
        assert_eq!(codec.decode(&mut buf).unwrap(), Some(frame));
        assert_eq!(codec.decode(&mut buf).unwrap(), None);
    }

    #[test]
    fn encode_prepends_wakeup_bytes() {
        let (frame, raw) = sample();
        let mut buf = BytesMut::new();
        Dl645Codec::new(4).encode(frame, &mut buf).unwrap();
        assert_eq!(&buf[..4], &[0xFE; 4]);
        assert_eq!(&buf[4..], &raw[..]);
    }
}
