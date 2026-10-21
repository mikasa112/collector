use calamine::{Data, DataType, HeaderRow, Range, Reader, Xlsx, open_workbook};
use collector_dlt645::{DataIdentifier, DiSpec, MeterAddress, spec_for};

use crate::{
    config::{optional_static_str, required_f64, required_hex, required_static_str},
    core::point::{Bits, DataPoint, PointId, Translator, Val, WarnLevel, Words},
};

#[derive(Debug, thiserror::Error)]
pub enum Dlt645ConfigsError {
    #[error("Failed to open workbook: {0}")]
    OpenWorkbookError(#[from] calamine::XlsxError),
}

/// 设备通讯状态点占用的 id，点表不可使用。
pub const COMM_STATUS_ID: PointId = 0xFFFF;

pub type Dlt645Configs = Vec<Dlt645Config>;

/// DL/T 645 点表中的一行。
///
/// Excel 工作表名 `dlt645`，第 1 行为表头，列依次为：
/// `id | key | name | address | di | scale | offset | unit | enable | trans | status_words | warn_bits | level | len | decimals | signed`
///
/// - `address` 电表地址(1~12 位十进制)，留空则使用设备配置里的 `meter_address`
/// - `di` 数据标识，8 位十六进制文本，例如 `02010100`(A 相电压)；请把单元格设为文本，避免前导 0 丢失
/// - `scale`/`offset`：`value = 解码值 * scale + offset`，留空默认 1 / 0。解码值已经应用了 DI 自带的小数位
/// - `unit` 留空时使用 DI 格式表里的单位
/// - `len`/`decimals`/`signed`：DI 在内置格式表里时可留空；不在表里(自定义 DI)时 `len` 必填，
///   `decimals` 默认 0、`signed` 默认 0；在表里时填了则覆盖内置值
/// - `trans`/`status_words`/`warn_bits`/`level` 含义与其它点表一致
#[derive(Debug, Clone)]
pub struct Dlt645Config {
    pub id: PointId,
    pub key: &'static str,
    pub name: &'static str,
    pub address: Option<MeterAddress>,
    pub spec: DiSpec,
    pub scale: f64,
    pub offset: f64,
    pub unit: Option<&'static str>,
    pub enable: bool,
    pub trans: Option<&'static Translator>,
    pub status_words: Option<&'static Words>,
    pub warn_bits: Option<&'static Bits>,
    pub level: Option<WarnLevel>,
}

impl Dlt645Config {
    /// 把解码后的工程值按 `scale`/`offset` 转成点值。
    pub fn to_val(&self, decoded: f64) -> Val {
        Val::F64(decoded * self.scale + self.offset)
    }

    pub fn to_data_point(&self, value: Val) -> DataPoint {
        DataPoint {
            id: self.id,
            key: self.key,
            name: self.name,
            value,
            translator: self.trans,
            bits: self.warn_bits,
            words: self.status_words,
            unit: self.unit,
            level: self.level,
        }
    }

    fn build(row: &[Data]) -> Result<Self, anyhow::Error> {
        let id = required_f64(row, 0, "id")? as PointId;
        if id == COMM_STATUS_ID {
            return Err(anyhow::anyhow!("id {COMM_STATUS_ID} 保留给通讯状态点"));
        }
        let key = required_static_str(row, 1, "KEY")?;
        let name = optional_static_str(row, 2).unwrap_or_default();
        let address = parse_address(row.get(3))?;
        if matches!(row.get(4), Some(Data::Float(_) | Data::Int(_))) {
            return Err(anyhow::anyhow!("DI请填写为文本(如 02010100)"));
        }
        let di = DataIdentifier::from_u32(required_hex(row, 4, "DI")?);
        let scale = row.get(5).and_then(|c| c.as_f64()).unwrap_or(1.0);
        if scale == 0.0 {
            return Err(anyhow::anyhow!("SCALE不能为0"));
        }
        let offset = row.get(6).and_then(|c| c.as_f64()).unwrap_or(0.0);
        let spec = build_spec(di, row)?;
        let unit = optional_static_str(row, 7)
            .filter(|s| !s.trim().is_empty())
            .or((!spec.unit.is_empty()).then_some(spec.unit));
        // 与其它点表一致：留空视为启用
        let enable = row.get(8).and_then(|c| c.as_f64()).unwrap_or(1.0) != 0.0;
        let trans = row
            .get(9)
            .and_then(|cell| cell.get_string())
            .and_then(|str| Translator::try_from(str).ok())
            .map(|t| &*Box::leak(Box::new(t)));
        let status_words = parse_optional_cell(row, 10, "STATUS_WORDS", |s| Words::try_from(s));
        let warn_bits = parse_optional_cell(row, 11, "WARN_BITS", |s| Bits::try_from(s));
        let level = row
            .get(12)
            .and_then(|cell| cell.as_f64())
            .map(|v| WarnLevel::from(v as u8))
            .filter(|lvl| *lvl != WarnLevel::None);
        Ok(Self {
            id,
            key,
            name,
            address,
            spec,
            scale,
            offset,
            unit,
            enable,
            trans,
            status_words,
            warn_bits,
            level,
        })
    }
}

/// 电表地址单元格：文本或数字都接受；留空返回 `None`。
fn parse_address(cell: Option<&Data>) -> Result<Option<MeterAddress>, anyhow::Error> {
    let text = match cell {
        None | Some(Data::Empty) => return Ok(None),
        Some(Data::String(s)) => s.trim().to_owned(),
        Some(Data::Float(v)) if v.is_finite() && v.fract() == 0.0 && *v >= 0.0 => {
            format!("{}", *v as u64)
        }
        Some(Data::Int(v)) if *v >= 0 => v.to_string(),
        Some(other) => return Err(anyhow::anyhow!("电表地址格式错误: {other:?}")),
    };
    if text.is_empty() {
        return Ok(None);
    }
    MeterAddress::parse(&text)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("电表地址 {text} 无效: {e}"))
}

/// 内置格式表 + 点表 `len/decimals/signed` 三列的覆盖。
fn build_spec(di: DataIdentifier, row: &[Data]) -> Result<DiSpec, anyhow::Error> {
    let cell_usize = |idx: usize, field: &str| -> Result<Option<usize>, anyhow::Error> {
        match row.get(idx) {
            None | Some(Data::Empty) => Ok(None),
            Some(cell) => match cell.as_f64() {
                Some(v) if v.is_finite() && v >= 0.0 && v.fract() == 0.0 => Ok(Some(v as usize)),
                _ => Err(anyhow::anyhow!("{field}必须是非负整数")),
            },
        }
    };
    let len = cell_usize(13, "LEN")?;
    let decimals = cell_usize(14, "DECIMALS")?;
    let signed = cell_usize(15, "SIGNED")?.map(|v| v != 0);
    let mut spec = match spec_for(di) {
        Some(spec) => spec,
        None => {
            let len = len.ok_or_else(|| {
                anyhow::anyhow!("DI {:08X} 不在内置格式表中, 必须填写LEN", di.as_u32())
            })?;
            DiSpec::new(di, len, 0, false, "")
        }
    };
    if let Some(len) = len {
        spec.len = len;
    }
    if let Some(decimals) = decimals {
        spec.decimals = u8::try_from(decimals).map_err(|_| anyhow::anyhow!("DECIMALS超出范围"))?;
    }
    if let Some(signed) = signed {
        spec.signed = signed;
    }
    if !(1..=8).contains(&spec.len) {
        return Err(anyhow::anyhow!("LEN需在1~8之间: {}", spec.len));
    }
    if spec.decimals > 9 {
        return Err(anyhow::anyhow!("DECIMALS需在0~9之间: {}", spec.decimals));
    }
    Ok(spec)
}

/// 解析可选的文本列：留空返回 `None`；有内容但解析失败时记录告警并返回 `None`。
fn parse_optional_cell<T>(
    row: &[Data],
    idx: usize,
    field: &str,
    parse: impl FnOnce(&str) -> Result<T, anyhow::Error>,
) -> Option<&'static T> {
    let text = row.get(idx)?.get_string()?;
    if text.trim().is_empty() {
        return None;
    }
    match parse(text) {
        Ok(v) => Some(&*Box::leak(Box::new(v))),
        Err(err) => {
            tracing::warn!("DLT645点表 {field} 解析失败, 已忽略: {err}");
            None
        }
    }
}

pub(crate) fn build_configs(path: String) -> Result<Dlt645Configs, Dlt645ConfigsError> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let mut configs = Vec::new();
    let parse = |range: Range<Data>, configs: &mut Vec<Dlt645Config>| {
        for row in range.rows() {
            match Dlt645Config::build(row) {
                Ok(config) => {
                    if configs.iter().any(|c: &Dlt645Config| c.id == config.id) {
                        tracing::error!("忽略DLT645点 {}: id {} 重复", config.key, config.id);
                    } else {
                        configs.push(config);
                    }
                }
                Err(err) => tracing::error!("构建DLT645配置失败: {}", err),
            }
        }
    };
    if let Ok(range) = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("dlt645")
    {
        parse(range, &mut configs);
    }
    Ok(configs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::Data::{Empty, Float, String as S};

    fn row(id: f64, address: Data, di: &str) -> Vec<Data> {
        vec![Float(id), S("k".into()), Empty, address, S(di.into())]
    }

    #[test]
    fn builtin_spec_unit_and_defaults() {
        let cfg = Dlt645Config::build(&row(1.0, S("000000000001".into()), "02010100")).unwrap();
        assert_eq!(cfg.address, Some(MeterAddress::parse("1").unwrap()));
        assert_eq!((cfg.spec.len, cfg.spec.decimals), (2, 1));
        assert_eq!(cfg.unit, Some("V"));
        assert!(cfg.enable);
        assert_eq!(cfg.to_val(2205.0 / 10.0), Val::F64(220.5));
    }

    #[test]
    fn numeric_address_cell_is_accepted_and_empty_means_default() {
        let cfg = Dlt645Config::build(&row(1.0, Float(123456789012.0), "00010000")).unwrap();
        assert_eq!(
            cfg.address,
            Some(MeterAddress::parse("123456789012").unwrap())
        );
        let cfg = Dlt645Config::build(&row(1.0, Empty, "00010000")).unwrap();
        assert_eq!(cfg.address, None);
    }

    #[test]
    fn scale_and_offset_apply_after_decimals() {
        let mut r = row(1.0, Empty, "00010000");
        r.extend([Float(0.001), Float(5.0)]);
        let cfg = Dlt645Config::build(&r).unwrap();
        assert_eq!(cfg.to_val(1000.0), Val::F64(6.0));
    }

    #[test]
    fn custom_di_requires_len_and_overrides_apply() {
        // 不在内置表里: 缺 LEN 报错
        assert!(Dlt645Config::build(&row(1.0, Empty, "04000401")).is_err());
        let mut r = row(1.0, Empty, "04000401");
        r.resize(13, Empty);
        r.extend([Float(6.0), Float(2.0), Float(1.0)]);
        let cfg = Dlt645Config::build(&r).unwrap();
        assert_eq!(
            (cfg.spec.len, cfg.spec.decimals, cfg.spec.signed),
            (6, 2, true)
        );
        // 内置 DI 上的覆盖
        let mut r = row(1.0, Empty, "02010100");
        r.resize(13, Empty);
        r.push(Float(3.0));
        assert_eq!(Dlt645Config::build(&r).unwrap().spec.len, 3);
    }

    #[test]
    fn rejects_bad_rows() {
        // 保留 id
        assert!(Dlt645Config::build(&row(65535.0, Empty, "00010000")).is_err());
        // 地址非法
        assert!(Dlt645Config::build(&row(1.0, S("12ab".into()), "00010000")).is_err());
        // DI 写成了数字单元格
        let mut r = row(1.0, Empty, "00010000");
        r[4] = Float(10000.0);
        assert!(Dlt645Config::build(&r).is_err());
        // scale = 0
        let mut r = row(1.0, Empty, "00010000");
        r.extend([Float(0.0)]);
        assert!(Dlt645Config::build(&r).is_err());
    }
}
