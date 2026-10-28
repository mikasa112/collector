use std::collections::HashMap;
use std::sync::Arc;

use calamine::{Data, DataType, HeaderRow, Range, Reader, Xlsx, open_workbook};

use crate::{
    config::{
        optional_shared_str, required_f64, required_shared_str, required_str,
        required_usize_integerish,
    },
    core::point::{Bits, DataPoint, PointId, Translator, Val, WarnLevel, Words},
};

#[derive(Debug, thiserror::Error)]
pub enum Iec104ConfigsError {
    #[error("Failed to open workbook: {0}")]
    OpenWorkbookError(#[from] calamine::XlsxError),
}

/// 设备通讯状态点占用的 id，点表不可使用。
pub const COMM_STATUS_ID: PointId = 0xFFFF;

pub type Iec104Configs = Vec<Iec104Config>;

/// 点表"类型"列的取值。
///
/// 监视方向（遥信/遥测）只需要 CA+IOA 即可匹配，具体 ASDU 类型由对端决定；
/// 控制方向需要在点表里明确命令类型，才能构造对应的控制 ASDU。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointKind {
    /// 遥信，单点/双点均可（M_SP_*、M_DP_*）
    Yx,
    /// 遥测，归一化/标度化/短浮点均可（M_ME_*）
    Yc,
    /// 单点遥控（C_SC_NA_1）
    Yk,
    /// 双点遥控（C_DC_NA_1）
    YkDouble,
    /// 短浮点设点（C_SE_NC_1）
    YtShort,
    /// 标度化设点（C_SE_NB_1）
    YtScaled,
    /// 归一化设点（C_SE_NA_1）
    YtNormalized,
    /// 步位置命令（C_RC_NA_1）
    YtStep,
}

impl PointKind {
    /// 是否为监视方向（上行）点。
    pub fn is_monitor(self) -> bool {
        matches!(self, PointKind::Yx | PointKind::Yc)
    }
}

impl TryFrom<&str> for PointKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "yx" => Ok(PointKind::Yx),
            "yc" => Ok(PointKind::Yc),
            "yk" => Ok(PointKind::Yk),
            "yk_double" => Ok(PointKind::YkDouble),
            "yt_short" => Ok(PointKind::YtShort),
            "yt_scaled" => Ok(PointKind::YtScaled),
            "yt_normalized" => Ok(PointKind::YtNormalized),
            "yt_step" => Ok(PointKind::YtStep),
            _ => Err(anyhow::anyhow!(
                "无效的点类型: {value}，期望 yx|yc|yk|yk_double|yt_short|yt_scaled|yt_normalized|yt_step"
            )),
        }
    }
}

/// IEC 104 点表中的一行。
///
/// Excel 工作表名 `iec104`，第 1 行为表头，列依次为：
/// `id | key | name | ca | ioa | type | scale | offset | unit | enable | trans | status_words | warn_bits | level`
///
/// - `ca` 公共地址，`ioa` 信息对象地址
/// - `scale`/`offset` 仅对遥测与设点生效：上送 `value = raw * scale + offset`，
///   下发 `raw = (value - offset) / scale`；留空时默认 1 / 0
/// - `status_words` 状态字翻译，每行 `值 中文|英文`；`warn_bits` 按位告警，必须正好 16 行，
///   每行 `中文|英文|等级(0~3)`。两列格式与 Modbus 点表一致，解析失败时记录告警并忽略该列
/// - `level` 单点告警等级：`0`/留空不告警，`1`/`2`/`3` 为 Normal/High/Critical；值非 0 即命中该等级
#[derive(Debug, Clone)]
pub struct Iec104Config {
    pub id: PointId,
    pub key: Arc<str>,
    pub name: Arc<str>,
    pub ca: i32,
    pub ioa: i32,
    pub kind: PointKind,
    pub scale: f64,
    pub offset: f64,
    pub unit: Option<Arc<str>>,
    pub enable: bool,
    pub trans: Option<Arc<Translator>>,
    pub status_words: Option<Arc<Words>>,
    pub warn_bits: Option<Arc<Bits>>,
    pub level: Option<WarnLevel>,
}

impl Iec104Config {
    pub fn to_data_point(&self, value: Val) -> DataPoint {
        DataPoint {
            id: self.id,
            key: self.key.clone(),
            name: self.name.clone(),
            value,
            translator: self.trans.clone(),
            bits: self.warn_bits.clone(),
            words: self.status_words.clone(),
            unit: self.unit.clone(),
            level: self.level,
        }
    }

    fn build(row: &[Data]) -> Result<Self, anyhow::Error> {
        let id = required_f64(row, 0, "id")? as PointId;
        if id == COMM_STATUS_ID {
            return Err(anyhow::anyhow!("id {COMM_STATUS_ID} 保留给通讯状态点"));
        }
        let key = required_shared_str(row, 1, "KEY")?;
        let name = optional_shared_str(row, 2).unwrap_or_default();
        let ca = i32::try_from(required_usize_integerish(row, 3, "CA")?)
            .map_err(|_| anyhow::anyhow!("CA超出范围"))?;
        let ioa = i32::try_from(required_usize_integerish(row, 4, "IOA")?)
            .map_err(|_| anyhow::anyhow!("IOA超出范围"))?;
        // CA 为 2 字节，0 无效、65535 为广播；IOA 为 3 字节，0 仅用于站级对象
        if !(1..=65534).contains(&ca) {
            return Err(anyhow::anyhow!("CA需在1~65534之间: {ca}"));
        }
        if !(1..=0xFF_FFFF).contains(&ioa) {
            return Err(anyhow::anyhow!("IOA需在1~16777215之间: {ioa}"));
        }
        let kind = PointKind::try_from(required_str(row, 5, "TYPE")?)?;
        let scale = row.get(6).and_then(|c| c.as_f64()).unwrap_or(1.0);
        if scale == 0.0 {
            return Err(anyhow::anyhow!("SCALE不能为0"));
        }
        let offset = row.get(7).and_then(|c| c.as_f64()).unwrap_or(0.0);
        let unit = optional_shared_str(row, 8);
        // 与 Modbus 点表一致：留空视为启用
        let enable = row.get(9).and_then(|c| c.as_f64()).unwrap_or(1.0) != 0.0;
        let trans = row
            .get(10)
            .and_then(|cell| cell.get_string())
            .and_then(|str| Translator::try_from(str).ok())
            .map(Arc::new);
        let status_words = parse_optional_cell(row, 11, "STATUS_WORDS", |s| Words::try_from(s));
        let warn_bits = parse_optional_cell(row, 12, "WARN_BITS", |s| Bits::try_from(s));
        let level = row
            .get(13)
            .and_then(|cell| cell.as_f64())
            .map(|v| WarnLevel::from(v as u8))
            .filter(|lvl| *lvl != WarnLevel::None);
        Ok(Self {
            id,
            key,
            name,
            ca,
            ioa,
            kind,
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

/// 解析可选的文本列：留空返回 `None`；有内容但解析失败时记录告警并返回 `None`。
fn parse_optional_cell<T>(
    row: &[Data],
    idx: usize,
    field: &str,
    parse: impl FnOnce(&str) -> Result<T, anyhow::Error>,
) -> Option<Arc<T>> {
    let text = row.get(idx)?.get_string()?;
    if text.trim().is_empty() {
        return None;
    }
    match parse(text) {
        Ok(v) => Some(Arc::new(v)),
        Err(err) => {
            tracing::warn!("IEC104点表 {field} 解析失败, 已忽略: {err}");
            None
        }
    }
}

pub(crate) fn build_configs(path: String) -> Result<Iec104Configs, Iec104ConfigsError> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let mut configs = Vec::new();
    let parse = |range: Range<Data>, configs: &mut Vec<Iec104Config>| {
        for row in range.rows() {
            match Iec104Config::build(row) {
                Ok(config) => match check_duplicate(configs, &config) {
                    None => configs.push(config),
                    Some(reason) => tracing::error!("忽略IEC104点 {}: {}", config.key, reason),
                },
                Err(err) => tracing::error!("构建IEC104配置失败: {}", err),
            }
        }
    };
    if let Ok(range) = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("iec104")
    {
        parse(range, &mut configs);
    }
    Ok(configs)
}

/// 检查新点是否与已有点冲突：id 必须唯一；同一 (CA, IOA) 只能有一个监视点。
/// 控制点允许与监视点共用 (CA, IOA)（用于回读），但控制点之间不能重复。
fn check_duplicate(existing: &[Iec104Config], new: &Iec104Config) -> Option<String> {
    existing.iter().find_map(|c| {
        if c.id == new.id {
            return Some(format!("id {} 重复", new.id));
        }
        let same_addr = c.ca == new.ca && c.ioa == new.ioa;
        let same_side = c.kind.is_monitor() == new.kind.is_monitor();
        (same_addr && same_side).then(|| format!("CA={} IOA={} 重复", new.ca, new.ioa))
    })
}

/// 监视点索引：(CA, IOA) -> 点配置。仅包含启用的遥信/遥测点。
pub fn build_monitor_index(configs: &[Iec104Config]) -> HashMap<(i32, i32), Iec104Config> {
    configs
        .iter()
        .filter(|c| c.enable && c.kind.is_monitor())
        .map(|c| ((c.ca, c.ioa), c.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sample_workbook() {
        let path = format!(
            "{}/../config/iec104/PCS_英博_协控.xlsx",
            env!("CARGO_MANIFEST_DIR")
        );
        let configs = build_configs(path).unwrap();
        assert_eq!(configs.len(), 15);
        let temp = configs
            .iter()
            .find(|c| c.key == "temperature".into())
            .unwrap();
        assert_eq!((temp.ca, temp.ioa, temp.kind), (1, 16388, PointKind::Yc));
        assert_eq!((temp.scale, temp.offset), (0.1, -40.0));
        assert_eq!(build_monitor_index(&configs).len(), 9);

        let breaker = configs
            .iter()
            .find(|c| c.key == "breakerStatus".into())
            .unwrap();
        let words = breaker.status_words.clone().expect("状态字应被解析");
        assert_eq!(words.0[&1].zh, "合闸".into());
        assert_eq!(words.0[&0].en, "Open".into());
        assert!(breaker.warn_bits.is_none());

        let trip = configs
            .iter()
            .find(|c| c.key == "faultWord".into())
            .unwrap();
        let bits = trip.warn_bits.clone().expect("告警位应被解析");
        assert_eq!(bits.bits[0].zh, "保护动作".into());
        assert_eq!(bits.bits[0].level, crate::core::point::WarnLevel::Critical);
        assert_eq!(bits.bits[15].level, crate::core::point::WarnLevel::None);

        // 配置会随点值一起进入 DataPoint
        let dp = trip.to_data_point(Val::U8(1));
        assert!(dp.bits.is_some() && dp.words.is_none());
        assert_eq!(dp.warning().len(), 1);

        // 单点告警等级
        let protect = configs
            .iter()
            .find(|c| c.key == "protectTrip".into())
            .unwrap();
        assert_eq!(protect.level, Some(WarnLevel::Critical));
        let dp = protect.to_data_point(Val::U8(1));
        assert_eq!(dp.active_alarm_level(), Some(WarnLevel::Critical));
        assert_eq!(protect.to_data_point(Val::U8(0)).active_alarm_level(), None);
        assert_eq!(breaker.level, None);
    }

    fn point(id: PointId, ca: i32, ioa: i32, kind: PointKind) -> Iec104Config {
        Iec104Config {
            id,
            key: "k".into(),
            name: "".into(),
            ca,
            ioa,
            kind,
            scale: 1.0,
            offset: 0.0,
            unit: None,
            enable: true,
            trans: None,
            status_words: None,
            warn_bits: None,
            level: None,
        }
    }

    #[test]
    fn reserved_id_and_default_enable() {
        use calamine::Data::{Float, String as S};
        let row = |id: f64, enable: Option<f64>| {
            let mut r = vec![
                Float(id),
                S("k".into()),
                Data::Empty,
                Float(1.0),
                Float(1.0),
                S("yx".into()),
                Data::Empty,
                Data::Empty,
                Data::Empty,
            ];
            r.push(enable.map(Float).unwrap_or(Data::Empty));
            r
        };
        assert!(Iec104Config::build(&row(65535.0, None)).is_err());
        assert!(Iec104Config::build(&row(1.0, None)).unwrap().enable);
        assert!(!Iec104Config::build(&row(1.0, Some(0.0))).unwrap().enable);
    }

    #[test]
    fn duplicate_checks() {
        let existing = [point(1, 1, 10, PointKind::Yc)];
        assert!(check_duplicate(&existing, &point(1, 1, 11, PointKind::Yc)).is_some());
        assert!(check_duplicate(&existing, &point(2, 1, 10, PointKind::Yx)).is_some());
        // 控制点可与监视点共用地址用于回读
        assert!(check_duplicate(&existing, &point(3, 1, 10, PointKind::YtShort)).is_none());
        assert!(check_duplicate(&existing, &point(4, 2, 10, PointKind::Yc)).is_none());
    }
}
