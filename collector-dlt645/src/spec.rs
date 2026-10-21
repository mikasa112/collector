use crate::{
    frame::DataIdentifier,
    point::{ValueError, decode_bcd_le, decode_signed_bcd_le},
};

/// 一个数据标识(DI)对应的数值格式。
///
/// DL/T 645-2007 的数值都是小端压缩 BCD,隐含固定小数位,例如电压 `XXX.X`、电流 `XXX.XXX`。
/// 有符号量的符号位在最高字节的 bit7。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiSpec {
    pub di: DataIdentifier,
    /// 数据域字节数(不含 4 字节 DI)
    pub len: usize,
    /// 隐含小数位数
    pub decimals: u8,
    /// 最高位是否为符号位
    pub signed: bool,
    pub unit: &'static str,
}

impl DiSpec {
    pub const fn new(
        di: DataIdentifier,
        len: usize,
        decimals: u8,
        signed: bool,
        unit: &'static str,
    ) -> Self {
        Self {
            di,
            len,
            decimals,
            signed,
            unit,
        }
    }

    /// 解码为未缩放的整数(例如 220.5V -> 2205),便于无损比较或自行缩放。
    pub fn decode_raw(&self, data: &[u8]) -> Result<i64, ValueError> {
        if data.len() != self.len {
            return Err(ValueError::LengthMismatch {
                expected: self.len,
                actual: data.len(),
            });
        }
        if self.signed {
            decode_signed_bcd_le(data)
        } else {
            let value = decode_bcd_le(data)?;
            i64::try_from(value).map_err(|_| ValueError::TooWide)
        }
    }

    /// 解码并按小数位缩放为工程值。
    pub fn decode(&self, data: &[u8]) -> Result<f64, ValueError> {
        Ok(self.decode_raw(data)? as f64 / 10f64.powi(self.decimals as i32))
    }
}

/// 查询常用 DI 的格式。未收录的 DI(分相电能、需量及发生时间、事件记录等)返回 `None`,
/// 调用方可以用 [`DiSpec::new`] 自行描述。
///
/// 目前收录:当前/历史电能(DI3=00)、电压、电流、功率、功率因数、相角(DI3=02)
/// 以及 `02 80 00 xx` 的零线电流、频率等。数据格式依据 DL/T 645-2007 附录 A,
/// 接入新型号电表时请用实表核对。
pub fn spec_for(di: DataIdentifier) -> Option<DiSpec> {
    let [di3, di2, di1, di0] = di.0;
    let mk = |len, decimals, signed, unit| Some(DiSpec::new(di, len, decimals, signed, unit));
    // 分相量:DI1 = 00 合相(或零序),01..03 = A/B/C 相
    let phase = di0 == 0x00 && di1 <= 0x03;
    let abc = di0 == 0x00 && (0x01..=0x03).contains(&di1);
    match (di3, di2) {
        // 电能 XXXXXX.XX:DI1 费率(00 总,01..3F),DI0 结算日(00 当前,01..0C 上 N 月)
        (0x00, 0x00..=0x0A) if di1 <= 0x3F && di0 <= 0x0C => {
            let unit = match di2 {
                0x00..=0x02 => "kWh",
                0x03..=0x08 => "kvarh",
                _ => "kVAh",
            };
            mk(4, 2, false, unit)
        }
        (0x02, 0x01) if abc => mk(2, 1, false, "V"),
        (0x02, 0x02) if phase => mk(3, 3, true, "A"),
        (0x02, 0x03) if phase => mk(3, 4, true, "kW"),
        (0x02, 0x04) if phase => mk(3, 4, true, "kvar"),
        (0x02, 0x05) if phase => mk(3, 4, true, "kVA"),
        (0x02, 0x06) if phase => mk(2, 3, true, ""),
        (0x02, 0x07) if abc => mk(2, 1, false, "°"),
        (0x02, 0x80) if di1 == 0x00 => match di0 {
            0x01 => mk(3, 3, true, "A"),
            0x02 => mk(2, 2, false, "Hz"),
            0x03..=0x04 => mk(3, 4, true, "kW"),
            0x05 => mk(3, 4, true, "kvar"),
            0x06 => mk(3, 4, true, "kVA"),
            0x07 => mk(2, 1, true, "℃"),
            0x08..=0x09 => mk(2, 2, false, "V"),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(di: u32) -> DiSpec {
        spec_for(DataIdentifier::from_u32(di)).unwrap()
    }

    #[test]
    fn energy_is_four_bytes_two_decimals() {
        let s = spec(0x0001_0000);
        assert_eq!((s.len, s.decimals, s.unit), (4, 2, "kWh"));
        // 12345.67 kWh
        assert_eq!(s.decode(&[0x67, 0x45, 0x23, 0x01]).unwrap(), 12345.67);
        assert_eq!(spec(0x0003_0000).unit, "kvarh");
        assert_eq!(spec(0x0009_0000).unit, "kVAh");
    }

    #[test]
    fn voltage_unsigned_one_decimal() {
        // 220.5 V
        assert_eq!(spec(0x0201_0100).decode(&[0x05, 0x22]).unwrap(), 220.5);
    }

    #[test]
    fn current_and_power_are_signed() {
        let current = spec(0x0202_0100);
        assert_eq!(current.decode(&[0x00, 0x15, 0x00]).unwrap(), 1.5);
        assert_eq!(current.decode(&[0x00, 0x15, 0x80]).unwrap(), -1.5);
        // -1.2345 kW
        assert_eq!(
            spec(0x0203_0000).decode(&[0x45, 0x23, 0x81]).unwrap(),
            -1.2345
        );
    }

    #[test]
    fn power_factor_negative() {
        assert_eq!(spec(0x0206_0000).decode(&[0x85, 0x89]).unwrap(), -0.985);
    }

    #[test]
    fn frequency() {
        assert_eq!(spec(0x0280_0002).decode(&[0x00, 0x50]).unwrap(), 50.0);
    }

    #[test]
    fn wrong_length_is_rejected() {
        assert_eq!(
            spec(0x0201_0100).decode(&[0x05]),
            Err(ValueError::LengthMismatch {
                expected: 2,
                actual: 1
            })
        );
    }

    #[test]
    fn unknown_or_out_of_range_di_returns_none() {
        assert!(spec_for(DataIdentifier::from_u32(0x0201_0400)).is_none());
        assert!(spec_for(DataIdentifier::from_u32(0x0001_4000)).is_none());
        assert!(spec_for(DataIdentifier::from_u32(0x0400_0101)).is_none());
    }
}
