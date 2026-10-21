use crate::MeterAddress;
use thiserror::Error;

pub const START: u8 = 0x68;
pub const END: u8 = 0x16;
pub const DATA_OFFSET: u8 = 0x33;
pub const CTRL_READ_DATA: u8 = 0x11;
pub const CTRL_READ_DATA_RESPONSE: u8 = 0x91;
pub const CTRL_EXCEPTION_BIT: u8 = 0x40;
pub const MAX_DATA_LEN: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
/// Stored in standard notation: DI3, DI2, DI1, DI0.
pub struct DataIdentifier(pub [u8; 4]);

impl DataIdentifier {
    pub const fn new(di3: u8, di2: u8, di1: u8, di0: u8) -> Self {
        Self([di3, di2, di1, di0])
    }
    pub const fn from_u32(value: u32) -> Self {
        Self([
            (value >> 24) as u8,
            (value >> 16) as u8,
            (value >> 8) as u8,
            value as u8,
        ])
    }
    pub const fn as_u32(self) -> u32 {
        ((self.0[0] as u32) << 24)
            | ((self.0[1] as u32) << 16)
            | ((self.0[2] as u32) << 8)
            | self.0[3] as u32
    }
    pub fn from_wire_data(data: &[u8]) -> Option<Self> {
        if data.len() < 4 {
            return None;
        }
        Some(Self([data[3], data[2], data[1], data[0]]))
    }
    pub const fn to_wire_data(self) -> [u8; 4] {
        [self.0[3], self.0[2], self.0[1], self.0[0]]
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub address: MeterAddress,
    pub control: u8,
    /// Logical (decoded) data bytes. The +0x33 transform is applied only during wire encoding.
    pub data: Vec<u8>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FrameError {
    #[error("data field exceeds DL/T 645-2007 maximum (200 bytes)")]
    DataTooLong,
    #[error("frame is too short")]
    TooShort,
    #[error("invalid first start byte")]
    InvalidStart,
    #[error("invalid second start byte")]
    InvalidSecondStart,
    #[error("invalid frame end byte")]
    InvalidEnd,
    #[error("frame length does not match data length")]
    LengthMismatch,
    #[error("checksum mismatch: expected {expected:02X}, received {received:02X}")]
    Checksum { expected: u8, received: u8 },
}

impl Frame {
    pub fn encode(&self) -> Result<Vec<u8>, FrameError> {
        if self.data.len() > MAX_DATA_LEN {
            return Err(FrameError::DataTooLong);
        }
        let mut out = Vec::with_capacity(12 + self.data.len());
        out.push(START);
        out.extend_from_slice(&self.address.to_wire());
        out.push(START);
        out.push(self.control);
        out.push(self.data.len() as u8);
        out.extend(self.data.iter().map(|b| b.wrapping_add(DATA_OFFSET)));
        let cs = checksum(&out);
        out.push(cs);
        out.push(END);
        Ok(out)
    }

    /// Parse exactly one frame, excluding optional FE wake-up bytes.
    pub fn decode(raw: &[u8]) -> Result<Self, FrameError> {
        if raw.len() < 12 {
            return Err(FrameError::TooShort);
        }
        if raw[0] != START {
            return Err(FrameError::InvalidStart);
        }
        if raw[7] != START {
            return Err(FrameError::InvalidSecondStart);
        }
        let data_len = raw[9] as usize;
        if data_len > MAX_DATA_LEN {
            return Err(FrameError::DataTooLong);
        }
        let expected_len = 12 + data_len;
        if raw.len() != expected_len {
            return Err(FrameError::LengthMismatch);
        }
        if raw[raw.len() - 1] != END {
            return Err(FrameError::InvalidEnd);
        }
        let cs_index = raw.len() - 2;
        let expected = checksum(&raw[..cs_index]);
        let received = raw[cs_index];
        if expected != received {
            return Err(FrameError::Checksum { expected, received });
        }
        let mut address = [0u8; 6];
        address.copy_from_slice(&raw[1..7]);
        let data = raw[10..cs_index]
            .iter()
            .map(|b| b.wrapping_sub(DATA_OFFSET))
            .collect();
        Ok(Self {
            address: MeterAddress::from_wire(address),
            control: raw[8],
            data,
        })
    }
}

pub fn checksum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0u8, |sum, b| sum.wrapping_add(*b))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_request_matches_known_frame() {
        let frame = Frame {
            address: MeterAddress::parse("123456789012").unwrap(),
            control: CTRL_READ_DATA,
            data: DataIdentifier::from_u32(0x00010000).to_wire_data().to_vec(),
        };
        assert_eq!(
            frame.encode().unwrap(),
            vec![
                0x68, 0x12, 0x90, 0x78, 0x56, 0x34, 0x12, 0x68, 0x11, 0x04, 0x33, 0x33, 0x34, 0x33,
                0x68, 0x16
            ]
        );
    }
    #[test]
    fn frame_round_trip() {
        let f = Frame {
            address: MeterAddress::parse("000000000001").unwrap(),
            control: 0x91,
            data: vec![0x00, 0x01, 0x02, 0x03, 0x12, 0x34],
        };
        assert_eq!(Frame::decode(&f.encode().unwrap()).unwrap(), f);
    }
    #[test]
    fn detects_checksum_error() {
        let f = Frame {
            address: MeterAddress::parse("1").unwrap(),
            control: 0x11,
            data: vec![0; 4],
        };
        let mut raw = f.encode().unwrap();
        raw[10] ^= 1;
        assert!(matches!(
            Frame::decode(&raw),
            Err(FrameError::Checksum { .. })
        ));
    }
}
