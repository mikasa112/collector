use std::convert::TryFrom;

use bytes::{BufMut, Bytes, BytesMut};

use super::checksum::crc32_ieee;
use super::error::FrameError;
use super::types::{FLAG_IS_COMPRESSED, Frame, Header, KNOWN_FLAGS_MASK, MAGIC, MsgType, VERSION};

const MAX_DECOMPRESSED_PAYLOAD_LEN: usize = 8 * 1024 * 1024;

impl TryFrom<u8> for MsgType {
    type Error = FrameError;

    fn try_from(value: u8) -> Result<Self, FrameError> {
        match value {
            1 => Ok(Self::Data),
            2 => Ok(Self::Control),
            5 => Ok(Self::Dict),
            6 => Ok(Self::Heartbeat),
            7 => Ok(Self::Ack),
            0xFF => Ok(Self::Error),
            _ => Err(FrameError::UnsupportedMsgType(value)),
        }
    }
}

impl Frame {
    pub fn maybe_compress_payload(
        &mut self,
        threshold: usize,
        level: i32,
    ) -> Result<(), FrameError> {
        if self.header.flags & FLAG_IS_COMPRESSED != 0 || self.payload.len() < threshold {
            return Ok(());
        }

        let compressed = zstd::bulk::compress(self.payload.as_ref(), level)
            .map_err(|e| FrameError::CompressionError(e.to_string()))?;
        if compressed.len() >= self.payload.len() {
            return Ok(());
        }

        self.payload = Bytes::from(compressed);
        self.header.flags |= FLAG_IS_COMPRESSED;
        self.header.payload_len = total_payload_len(self.device_id.len(), self.payload.len())?;
        self.crc = self.calc_crc();
        Ok(())
    }

    pub(crate) fn new(
        msg_type: MsgType,
        flags: u16,
        seq: u32,
        timestamp_ms: u64,
        device_id: impl Into<Bytes>,
        payload: impl Into<Bytes>,
    ) -> Result<Self, FrameError> {
        let device_id = device_id.into();
        let payload = payload.into();
        if device_id.len() > u16::MAX as usize {
            return Err(FrameError::FieldTooLarge);
        }
        let total_payload_len = total_payload_len(device_id.len(), payload.len())?;

        let header = Header {
            magic: MAGIC,
            version: VERSION,
            msg_type,
            flags,
            header_len: Header::FIXED_LEN as u16,
            payload_len: total_payload_len,
            seq,
            timestamp_ms,
            device_id_len: device_id.len() as u16,
        };
        let mut frame = Self {
            header,
            device_id,
            payload,
            crc: 0,
        };
        frame.crc = frame.calc_crc();
        Ok(frame)
    }

    pub fn encoded_len(&self) -> usize {
        self.header.header_len as usize + self.header.payload_len as usize + 4
    }

    pub fn encode(&self) -> Result<BytesMut, FrameError> {
        if self.device_id.len() != self.header.device_id_len as usize {
            return Err(FrameError::DeviceIdLengthMismatch);
        }
        if self.device_id.len() + self.payload.len() != self.header.payload_len as usize {
            return Err(FrameError::FrameLengthMismatch);
        }
        if self.header.header_len as usize != Header::FIXED_LEN {
            return Err(FrameError::HeaderLengthOutOfRange(self.header.header_len));
        }

        let mut out = BytesMut::with_capacity(self.encoded_len());
        out.put_u16(self.header.magic);
        out.put_u8(self.header.version);
        out.put_u8(self.header.msg_type as u8);
        out.put_u16(self.header.flags);
        out.put_u16(self.header.header_len);
        out.put_u32(self.header.payload_len);
        out.put_u32(self.header.seq);
        out.put_u64(self.header.timestamp_ms);
        out.put_u16(self.header.device_id_len);
        out.extend_from_slice(&self.device_id);
        out.extend_from_slice(&self.payload);

        let crc = crc32_ieee(&out[..]);
        out.put_u32(crc);
        Ok(out)
    }

    pub fn peek_frame_len(buf: &[u8]) -> Result<Option<usize>, FrameError> {
        if buf.len() < Header::FIXED_LEN {
            return Ok(None);
        }
        let magic = u16::from_be_bytes([buf[0], buf[1]]);
        if magic != MAGIC {
            return Err(FrameError::BadMagic(magic));
        }
        let version = buf[2];
        if version != VERSION {
            return Err(FrameError::UnsupportedVersion(version));
        }
        let header_len = u16::from_be_bytes([buf[6], buf[7]]);
        if header_len as usize != Header::FIXED_LEN {
            return Err(FrameError::HeaderLengthOutOfRange(header_len));
        }
        let payload_len = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]) as usize;
        Ok(Some(header_len as usize + payload_len + 4))
    }

    pub fn decode(buf: &[u8]) -> Result<Self, FrameError> {
        let Some(expected_len) = Self::peek_frame_len(buf)? else {
            return Err(FrameError::FrameTooShort);
        };
        if buf.len() < expected_len {
            return Err(FrameError::FrameTooShort);
        }
        if buf.len() != expected_len {
            return Err(FrameError::FrameLengthMismatch);
        }

        let msg_type = MsgType::try_from(buf[3])?;
        let flags = u16::from_be_bytes([buf[4], buf[5]]);
        let header_len = u16::from_be_bytes([buf[6], buf[7]]);
        let payload_len = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);
        let seq = u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]);
        let timestamp_ms = u64::from_be_bytes([
            buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23],
        ]);
        let device_id_len = u16::from_be_bytes([buf[24], buf[25]]) as usize;

        let body_len = expected_len - 4;
        let actual_crc = u32::from_be_bytes([
            buf[body_len],
            buf[body_len + 1],
            buf[body_len + 2],
            buf[body_len + 3],
        ]);
        let expected_crc = crc32_ieee(&buf[..body_len]);
        if actual_crc != expected_crc {
            return Err(FrameError::CrcMismatch {
                expected: expected_crc,
                actual: actual_crc,
            });
        }

        let payload_start = header_len as usize;
        let payload_end = payload_start + payload_len as usize;
        if payload_end != body_len {
            return Err(FrameError::FrameLengthMismatch);
        }
        if device_id_len > payload_len as usize {
            return Err(FrameError::DeviceIdLengthMismatch);
        }

        let device_id_end = payload_start + device_id_len;
        let device_id = Bytes::copy_from_slice(&buf[payload_start..device_id_end]);
        let mut payload = Bytes::copy_from_slice(&buf[device_id_end..payload_end]);
        let mut normalized_flags = flags;
        let mut normalized_payload_len = payload_len;

        if flags & FLAG_IS_COMPRESSED != 0 {
            let decompressed =
                zstd::bulk::decompress(payload.as_ref(), MAX_DECOMPRESSED_PAYLOAD_LEN)
                    .map_err(|e| FrameError::CompressionError(e.to_string()))?;
            payload = Bytes::from(decompressed);
            normalized_flags &= !FLAG_IS_COMPRESSED;
            normalized_payload_len = total_payload_len(device_id_len, payload.len())?;
        }

        let mut frame = Self {
            header: Header {
                magic: MAGIC,
                version: VERSION,
                msg_type,
                flags: normalized_flags,
                header_len,
                payload_len: normalized_payload_len,
                seq,
                timestamp_ms,
                device_id_len: device_id_len as u16,
            },
            device_id,
            payload,
            crc: actual_crc,
        };
        frame.crc = frame.calc_crc();
        Ok(frame)
    }

    pub fn has_unknown_flags(&self) -> bool {
        self.header.flags & !KNOWN_FLAGS_MASK != 0
    }

    fn calc_crc(&self) -> u32 {
        let mut head_and_body = BytesMut::with_capacity(self.encoded_len().saturating_sub(4));
        head_and_body.put_u16(self.header.magic);
        head_and_body.put_u8(self.header.version);
        head_and_body.put_u8(self.header.msg_type as u8);
        head_and_body.put_u16(self.header.flags);
        head_and_body.put_u16(self.header.header_len);
        head_and_body.put_u32(self.header.payload_len);
        head_and_body.put_u32(self.header.seq);
        head_and_body.put_u64(self.header.timestamp_ms);
        head_and_body.put_u16(self.header.device_id_len);
        head_and_body.extend_from_slice(&self.device_id);
        head_and_body.extend_from_slice(&self.payload);
        crc32_ieee(&head_and_body[..])
    }
}

fn total_payload_len(device_id_len: usize, payload_len: usize) -> Result<u32, FrameError> {
    let total = device_id_len
        .checked_add(payload_len)
        .ok_or(FrameError::FieldTooLarge)?;
    u32::try_from(total).map_err(|_| FrameError::FieldTooLarge)
}
