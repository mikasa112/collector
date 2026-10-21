use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub enum PointValue {
    Unsigned(u64),
    Bcd(u64),
    Bytes(Vec<u8>),
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ValueError {
    #[error("empty value")]
    Empty,
    #[error("value is too wide for u64")]
    TooWide,
    #[error("invalid BCD digit 0x{0:X}")]
    InvalidBcd(u8),
    #[error("value length mismatch: expected {expected} bytes, got {actual}")]
    LengthMismatch { expected: usize, actual: usize },
}

/// Interpret little-endian binary bytes (least significant byte first).
pub fn decode_unsigned_le(bytes: &[u8]) -> Result<u64, ValueError> {
    if bytes.is_empty() {
        return Err(ValueError::Empty);
    }
    if bytes.len() > 8 {
        return Err(ValueError::TooWide);
    }
    let mut value = 0u64;
    for (i, byte) in bytes.iter().enumerate() {
        value |= (*byte as u64) << (i * 8);
    }
    Ok(value)
}

/// Interpret little-endian packed BCD bytes; low nibble is the first decimal digit.
/// Example wire-decoded bytes [0x45, 0x23, 0x01] => decimal digits 12345.
pub fn decode_bcd_le(bytes: &[u8]) -> Result<u64, ValueError> {
    if bytes.is_empty() {
        return Err(ValueError::Empty);
    }
    let mut value = 0u64;
    let mut multiplier = 1u64;
    for byte in bytes {
        let low = byte & 0x0f;
        let high = byte >> 4;
        if low > 9 {
            return Err(ValueError::InvalidBcd(low));
        }
        if high > 9 {
            return Err(ValueError::InvalidBcd(high));
        }
        value = value
            .checked_add(
                (low as u64)
                    .checked_mul(multiplier)
                    .ok_or(ValueError::TooWide)?,
            )
            .ok_or(ValueError::TooWide)?;
        multiplier = multiplier.checked_mul(10).ok_or(ValueError::TooWide)?;
        value = value
            .checked_add(
                (high as u64)
                    .checked_mul(multiplier)
                    .ok_or(ValueError::TooWide)?,
            )
            .ok_or(ValueError::TooWide)?;
        multiplier = multiplier.checked_mul(10).ok_or(ValueError::TooWide)?;
    }
    Ok(value)
}

/// 解析带符号的小端压缩 BCD:最高字节的 bit7 为符号位(1 = 负),其余位为数值。
/// DL/T 645-2007 中电流、功率、功率因数等量使用这种格式。
pub fn decode_signed_bcd_le(bytes: &[u8]) -> Result<i64, ValueError> {
    if bytes.is_empty() {
        return Err(ValueError::Empty);
    }
    if bytes.len() > 8 {
        return Err(ValueError::TooWide);
    }
    let negative = bytes[bytes.len() - 1] & 0x80 != 0;
    let mut magnitude = bytes.to_vec();
    *magnitude.last_mut().expect("non-empty") &= 0x7F;
    let magnitude = i64::try_from(decode_bcd_le(&magnitude)?).map_err(|_| ValueError::TooWide)?;
    Ok(if negative { -magnitude } else { magnitude })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unsigned_little_endian() {
        assert_eq!(decode_unsigned_le(&[0x34, 0x12]).unwrap(), 0x1234);
    }
    #[test]
    fn bcd_little_endian() {
        assert_eq!(decode_bcd_le(&[0x45, 0x23, 0x01]).unwrap(), 12345);
    }
    #[test]
    fn invalid_bcd_rejected() {
        assert_eq!(decode_bcd_le(&[0xFA]), Err(ValueError::InvalidBcd(0xA)));
    }
    #[test]
    fn signed_bcd() {
        assert_eq!(decode_signed_bcd_le(&[0x00, 0x15, 0x00]).unwrap(), 1500);
        assert_eq!(decode_signed_bcd_le(&[0x00, 0x15, 0x80]).unwrap(), -1500);
        assert_eq!(decode_signed_bcd_le(&[0x00, 0x80]).unwrap(), 0);
        assert_eq!(
            decode_signed_bcd_le(&[0x0A, 0x00]),
            Err(ValueError::InvalidBcd(0xA))
        );
    }
}
