use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MeterAddress([u8; 6]);

#[derive(Debug, Error, PartialEq, Eq)]
pub enum AddressError {
    #[error("meter address must contain 1..=12 decimal digits")]
    InvalidLength,
    #[error("meter address contains a non-decimal character")]
    InvalidDigit,
}

impl MeterAddress {
    /// Construct from the human-readable 1..=12 digit address. Leading zeroes are preserved
    /// only if present in the input; the wire representation is always six BCD bytes, low byte first.
    pub fn parse(s: &str) -> Result<Self, AddressError> {
        if s.is_empty() || s.len() > 12 {
            return Err(AddressError::InvalidLength);
        }
        if !s.bytes().all(|b| b.is_ascii_digit()) {
            return Err(AddressError::InvalidDigit);
        }
        let padded = format!("{:0>12}", s);
        let bytes = padded.as_bytes();
        let mut wire = [0u8; 6];
        for i in 0..6 {
            let hi = bytes[10 - i * 2] - b'0';
            let lo = bytes[11 - i * 2] - b'0';
            wire[i] = (hi << 4) | lo;
        }
        Ok(Self(wire))
    }

    pub const fn from_wire(wire: [u8; 6]) -> Self {
        Self(wire)
    }
    pub const fn to_wire(&self) -> [u8; 6] {
        self.0
    }

    pub fn to_digits(&self) -> Result<String, AddressError> {
        let mut s = String::with_capacity(12);
        for byte in self.0.iter().rev() {
            let hi = byte >> 4;
            let lo = byte & 0x0f;
            if hi > 9 || lo > 9 {
                return Err(AddressError::InvalidDigit);
            }
            s.push(char::from(b'0' + hi));
            s.push(char::from(b'0' + lo));
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn address_wire_order() {
        let a = MeterAddress::parse("123456789012").unwrap();
        assert_eq!(a.to_wire(), [0x12, 0x90, 0x78, 0x56, 0x34, 0x12]);
        assert_eq!(a.to_digits().unwrap(), "123456789012");
    }
    #[test]
    fn short_address_is_zero_padded() {
        assert_eq!(
            MeterAddress::parse("1").unwrap().to_wire(),
            [1, 0, 0, 0, 0, 0]
        );
    }
}
