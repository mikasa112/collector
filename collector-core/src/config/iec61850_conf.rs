use std::collections::HashSet;

use calamine::{Data, DataType, HeaderRow, Reader, Xlsx, open_workbook};
use collector_iec61850::{
    Fc, ObjectRef, TRG_DATA_CHANGED, TRG_DATA_UPDATE, TRG_GI, TRG_INTEGRITY, TRG_QUALITY_CHANGED,
};

use crate::{
    config::{
        optional_static_str, required_f64, required_static_str, required_str,
        required_usize_integerish,
    },
    core::point::{Bits, DataPoint, PointId, Translator, Val, WarnLevel, Words},
};

#[derive(Debug, thiserror::Error)]
pub enum Iec61850ConfigsError {
    #[error("Failed to open workbook: {0}")]
    OpenWorkbookError(#[from] calamine::XlsxError),
}

/// 设备通讯状态点占用的 id，点表不可使用。
pub const COMM_STATUS_ID: PointId = 0xFFFF;

/// 点表"type"列的取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointKind {
    /// 遥信：布尔、枚举、整数或双点位置
    Yx,
    /// 遥测：浮点或整数，`原始值 × scale + offset`
    Yc,
    /// 品质 `q`
    Yq,
    /// 时标 `t`
    Ytime,
    /// 遥控：`ctlVal` 为布尔
    Yk,
    /// 遥调：`ctlVal` 为数值（浮点 APC 等）
    Yt,
}

impl PointKind {
    /// 是否为监视方向（上行）点。
    pub fn is_monitor(self) -> bool {
        !self.is_control()
    }

    pub fn is_control(self) -> bool {
        matches!(self, PointKind::Yk | PointKind::Yt)
    }
}

impl TryFrom<&str> for PointKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "yx" => Ok(PointKind::Yx),
            "yc" => Ok(PointKind::Yc),
            "yq" => Ok(PointKind::Yq),
            "ytime" => Ok(PointKind::Ytime),
            "yk" => Ok(PointKind::Yk),
            "yt" => Ok(PointKind::Yt),
            _ => Err(anyhow::anyhow!(
                "无效的点类型: {value}，期望 yx|yc|yq|ytime|yk|yt"
            )),
        }
    }
}

/// 控制点的附加选项（点表 `ctl_opts` 列，逗号分隔）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CtlOpts {
    /// 联锁检查
    pub interlock: bool,
    /// 同期检查
    pub synchro: bool,
    /// 测试位：IED 只校验不真正动作
    pub test: bool,
}

impl CtlOpts {
    fn parse(text: &str) -> Result<Self, anyhow::Error> {
        let mut opts = Self::default();
        for token in text.split(',').map(|t| t.trim().to_ascii_lowercase()) {
            match token.as_str() {
                "" => {}
                "interlock" => opts.interlock = true,
                "synchro" => opts.synchro = true,
                "test" => opts.test = true,
                other => {
                    return Err(anyhow::anyhow!(
                        "无效的控制选项: {other}，期望 interlock|synchro|test"
                    ));
                }
            }
        }
        Ok(opts)
    }
}

/// IEC 61850 点表 `iec61850` sheet 中的一行。
///
/// 第 1 行为表头，列依次为：
/// `id | key | name | ref | fc | type | scale | offset | unit | enable | trans | status_words |
/// warn_bits | level | ctl_opts | feedback`
///
/// - `ref` 对象引用，`LD/LN.DO.DA` 或 `LD/LN$FC$DO$DA`。监视点必须指向叶子数据属性，
///   控制点指向控制对象 `LD/LN.DO`。LD 部分可以只写后缀（如 `MONT` 匹配 `PCS0001MONT`）
/// - `fc` 监视点必填（也可以直接写在 `ref` 里）；控制点留空
/// - `scale`/`offset` 仅对遥测与遥调生效：上送 `value = raw * scale + offset`，
///   下发 `raw = (value - offset) / scale`；留空时默认 1 / 0
/// - `feedback` 控制点的回读点 key。控制命令的应答成功不代表设备已执行，要靠回读点确认
/// - 其余列的含义与其他协议点表一致
#[derive(Debug, Clone)]
pub struct Iec61850Config {
    pub id: PointId,
    pub key: &'static str,
    pub name: &'static str,
    /// 监视点带 FC；控制点不带
    pub reference: ObjectRef,
    pub kind: PointKind,
    pub scale: f64,
    pub offset: f64,
    pub unit: Option<&'static str>,
    pub enable: bool,
    pub trans: Option<&'static Translator>,
    pub status_words: Option<&'static Words>,
    pub warn_bits: Option<&'static Bits>,
    pub level: Option<WarnLevel>,
    pub ctl_opts: CtlOpts,
    pub feedback: Option<&'static str>,
}

impl Iec61850Config {
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
        let mut reference: ObjectRef = required_str(row, 3, "REF")?
            .parse()
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let fc_col = trimmed(row, 4)
            .map(|s| s.parse::<Fc>().map_err(|e| anyhow::anyhow!(e)))
            .transpose()?;
        let kind = PointKind::try_from(required_str(row, 5, "TYPE")?)?;

        if kind.is_monitor() {
            if reference.path.len() < 2 {
                return Err(anyhow::anyhow!(
                    "监视点必须指向叶子数据属性(LD/LN.DO.DA): {reference}"
                ));
            }
            let fc = match (reference.fc, fc_col) {
                (Some(a), Some(b)) if a != b => {
                    return Err(anyhow::anyhow!("ref 里的 FC({a:?}) 与 fc 列({b:?}) 不一致"));
                }
                (a, b) => a
                    .or(b)
                    .ok_or_else(|| anyhow::anyhow!("监视点的FC不能为空"))?,
            };
            reference.fc = Some(fc);
        } else {
            // 控制点只写到控制对象, FC 固定为 CO, 由库内部处理
            reference.fc = None;
        }

        let scale = row.get(6).and_then(|c| c.as_f64()).unwrap_or(1.0);
        if scale == 0.0 {
            return Err(anyhow::anyhow!("SCALE不能为0"));
        }
        let offset = row.get(7).and_then(|c| c.as_f64()).unwrap_or(0.0);
        let unit = optional_static_str(row, 8);
        // 与其他点表一致：留空视为启用
        let enable = row.get(9).and_then(|c| c.as_f64()).unwrap_or(1.0) != 0.0;
        let trans = row
            .get(10)
            .and_then(|cell| cell.get_string())
            .and_then(|str| Translator::try_from(str).ok())
            .map(|t| &*Box::leak(Box::new(t)));
        let status_words = parse_optional_cell(row, 11, "STATUS_WORDS", |s| Words::try_from(s));
        let warn_bits = parse_optional_cell(row, 12, "WARN_BITS", |s| Bits::try_from(s));
        let level = row
            .get(13)
            .and_then(|cell| cell.as_f64())
            .map(|v| WarnLevel::from(v as u8))
            .filter(|lvl| *lvl != WarnLevel::None);
        let ctl_opts = match trimmed(row, 14) {
            Some(text) if kind.is_control() => CtlOpts::parse(text)?,
            Some(_) => {
                tracing::warn!("IEC61850点表 {key}: 监视点的 ctl_opts 无效, 已忽略");
                CtlOpts::default()
            }
            None => CtlOpts::default(),
        };
        let feedback = optional_static_str(row, 15)
            .map(|s| s.trim())
            .filter(|s| !s.is_empty());

        Ok(Self {
            id,
            key,
            name,
            reference,
            kind,
            scale,
            offset,
            unit,
            enable,
            trans,
            status_words,
            warn_bits,
            level,
            ctl_opts,
            feedback,
        })
    }
}

fn trimmed(row: &[Data], idx: usize) -> Option<&str> {
    row.get(idx)
        .and_then(|c| c.get_string())
        .map(str::trim)
        .filter(|s| !s.is_empty())
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
            tracing::warn!("IEC61850点表 {field} 解析失败, 已忽略: {err}");
            None
        }
    }
}

/// 取数来源的类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// 非缓冲报告控制块
    Urcb,
    /// 缓冲报告控制块
    Brcb,
    /// 周期读取一个数据集
    Dataset,
}

/// IEC 61850 点表 `source` sheet 中的一行：一个报告控制块，或一个周期轮询的数据集。
///
/// 第 1 行为表头，列依次为：
/// `kind | ref | dataset | trg | intg_pd | buf_tm | interval | gi | purge | enable`
///
/// - `ref` RCB 引用（`LD/LN.RP.name` 或 `LD/LN.BR.name`，`name` 末尾的 `*` 通配所有实例，
///   设备上可复用的 RCB 通常有 `name01`~`nn` 多个实例）；`dataset` 类型则是数据集引用
/// - `trg` 触发条件，逗号分隔 `dchg,qchg,dupd,period`；留空用默认的 `dchg,qchg`
/// - `interval` RCB 定期重新触发总召的周期，`dataset` 的轮询周期；毫秒，留空用设备的 `interval`，
///   `0` 表示不重复
/// - `purge` 仅 BRCB：使能时清空 IED 里的缓冲，默认 `0`，即补发断线期间的事件
#[derive(Debug, Clone, PartialEq)]
pub struct Iec61850Source {
    pub kind: SourceKind,
    pub reference: String,
    pub dataset: Option<String>,
    /// `TRG_*` 的按位或；`None` 用库的默认值
    pub trg_ops: Option<u32>,
    pub intg_pd_ms: Option<u32>,
    pub buf_tm_ms: Option<u32>,
    pub interval_ms: Option<u64>,
    pub gi: bool,
    pub purge: bool,
    pub enable: bool,
}

impl Iec61850Source {
    fn build(row: &[Data]) -> Result<Self, anyhow::Error> {
        let kind = match required_str(row, 0, "KIND")?.trim().to_ascii_lowercase().as_str() {
            "urcb" => SourceKind::Urcb,
            "brcb" => SourceKind::Brcb,
            "dataset" => SourceKind::Dataset,
            other => {
                return Err(anyhow::anyhow!("无效的来源类型: {other}，期望 urcb|brcb|dataset"));
            }
        };
        let reference = required_str(row, 1, "REF")?.trim().to_string();
        let normalized = reference.replace('$', ".");
        if !reference.contains('/') {
            return Err(anyhow::anyhow!("REF缺少LD与LN之间的'/': {reference}"));
        }
        match kind {
            SourceKind::Urcb if !normalized.contains(".RP.") => {
                return Err(anyhow::anyhow!("urcb 的 REF 应为 LD/LN.RP.name: {reference}"));
            }
            SourceKind::Brcb if !normalized.contains(".BR.") => {
                return Err(anyhow::anyhow!("brcb 的 REF 应为 LD/LN.BR.name: {reference}"));
            }
            _ => {}
        }
        let trg_ops = trimmed(row, 3).map(parse_trg).transpose()?;
        let millis = |idx: usize, field: &str| -> Result<Option<u64>, anyhow::Error> {
            match row.get(idx) {
                Some(Data::Empty) | None => Ok(None),
                Some(_) => required_usize_integerish(row, idx, field).map(|v| Some(v as u64)),
            }
        };
        let to_u32 = |v: Option<u64>, field: &str| {
            v.map(|v| u32::try_from(v).map_err(|_| anyhow::anyhow!("{field}超出范围")))
                .transpose()
        };
        let flag = |idx: usize, default: bool| {
            row.get(idx)
                .and_then(|c| c.as_f64())
                .map(|v| v != 0.0)
                .unwrap_or(default)
        };
        Ok(Self {
            kind,
            reference,
            dataset: trimmed(row, 2).map(str::to_string),
            trg_ops,
            intg_pd_ms: to_u32(millis(4, "INTG_PD")?, "INTG_PD")?,
            buf_tm_ms: to_u32(millis(5, "BUF_TM")?, "BUF_TM")?,
            interval_ms: millis(6, "INTERVAL")?,
            gi: flag(7, true),
            purge: flag(8, false),
            enable: flag(9, true),
        })
    }
}

fn parse_trg(text: &str) -> Result<u32, anyhow::Error> {
    let mut mask = 0;
    for token in text.split(',').map(|t| t.trim().to_ascii_lowercase()) {
        mask |= match token.as_str() {
            "" => 0,
            "dchg" => TRG_DATA_CHANGED,
            "qchg" => TRG_QUALITY_CHANGED,
            "dupd" => TRG_DATA_UPDATE,
            "period" => TRG_INTEGRITY,
            "gi" => TRG_GI,
            other => {
                return Err(anyhow::anyhow!(
                    "无效的触发条件: {other}，期望 dchg|qchg|dupd|period"
                ));
            }
        };
    }
    Ok(mask)
}

/// 一台 IEC 61850 设备的全部配置：点表加取数来源。
#[derive(Debug, Clone, Default)]
pub struct Iec61850Configs {
    pub points: Vec<Iec61850Config>,
    pub sources: Vec<Iec61850Source>,
}

pub(crate) fn build_configs(path: String) -> Result<Iec61850Configs, Iec61850ConfigsError> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let mut configs = Iec61850Configs::default();

    if let Ok(range) = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("iec61850")
    {
        for row in range.rows() {
            match Iec61850Config::build(row) {
                Ok(config) => match check_duplicate(&configs.points, &config) {
                    None => configs.points.push(config),
                    Some(reason) => tracing::error!("忽略IEC61850点 {}: {}", config.key, reason),
                },
                Err(err) => tracing::error!("构建IEC61850配置失败: {}", err),
            }
        }
    }
    if let Ok(range) = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range("source")
    {
        for row in range.rows() {
            match Iec61850Source::build(row) {
                Ok(source) => {
                    if configs.sources.iter().any(|s| s.reference == source.reference) {
                        tracing::error!("忽略IEC61850取数来源 {}: 重复", source.reference);
                    } else {
                        configs.sources.push(source);
                    }
                }
                Err(err) => tracing::error!("构建IEC61850取数来源失败: {}", err),
            }
        }
    }
    check_feedback(&mut configs.points);
    Ok(configs)
}

/// 检查新点是否与已有点冲突：id 与 key 必须唯一；同一个叶子属性只能有一个监视点，
/// 同一个控制对象只能有一个控制点。监视点和控制点各自独立（回读点的引用本来就不同）。
fn check_duplicate(existing: &[Iec61850Config], new: &Iec61850Config) -> Option<String> {
    existing.iter().find_map(|c| {
        if c.id == new.id {
            return Some(format!("id {} 重复", new.id));
        }
        if c.key == new.key {
            return Some(format!("key {} 重复", new.key));
        }
        (c.kind.is_monitor() == new.kind.is_monitor() && c.reference == new.reference)
            .then(|| format!("{} 重复", new.reference))
    })
}

/// 控制点的回读点必须是点表里存在的监视点，否则无法确认命令是否生效。
fn check_feedback(points: &mut [Iec61850Config]) {
    let monitors: HashSet<&'static str> = points
        .iter()
        .filter(|p| p.kind.is_monitor())
        .map(|p| p.key)
        .collect();
    for p in points.iter_mut().filter(|p| p.kind.is_control()) {
        match p.feedback {
            Some(fb) if !monitors.contains(fb) => {
                tracing::warn!("IEC61850点 {}: 回读点 {fb} 不存在, 已忽略", p.key);
                p.feedback = None;
            }
            None => tracing::warn!(
                "IEC61850点 {}: 没有回读点, 命令应答成功后无法确认设备是否执行",
                p.key
            ),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calamine::Data::{Empty, Float, String as S};

    fn row(cells: &[Data]) -> Vec<Data> {
        cells.to_vec()
    }

    fn manifest(rel: &str) -> String {
        format!("{}/../{rel}", env!("CARGO_MANIFEST_DIR"))
    }

    #[test]
    fn parse_sample_workbook() {
        let configs = build_configs(manifest("config/iec61850/iec61850_示例点表.xlsx")).unwrap();
        let count = |k: PointKind| configs.points.iter().filter(|p| p.kind == k).count();
        assert_eq!(
            [PointKind::Yx, PointKind::Yc, PointKind::Yk, PointKind::Yt].map(count),
            [28, 116, 6, 41]
        );
        assert_eq!(configs.points.len(), 191);
        let by_key = |k: &str| configs.points.iter().find(|p| p.key == k).unwrap();

        // 嵌套对象展开到叶子, FC 来自 fc 列
        let ab = by_key("MMXU1_PPV_phsAB");
        assert_eq!(ab.reference.to_dotted(), "MONT/MMXU1.PPV.phsAB.cVal.mag.f");
        assert_eq!((ab.reference.fc, ab.kind), (Some(Fc::Mx), PointKind::Yc));
        assert_eq!(ab.name, "线电压-phsAB");

        // 控制点: 引用到控制对象, 没有 FC, 带回读点
        let cmd = by_key("setGAPC1_StrVal6_cmd");
        assert_eq!((cmd.reference.to_dotted().as_str(), cmd.reference.fc), ("MONT/setGAPC1.StrVal6", None));
        assert_eq!((cmd.kind, cmd.feedback, cmd.enable), (PointKind::Yt, Some("setGAPC1_StrVal6"), true));
        assert!(by_key("setGAPC1_StrVal6").kind.is_monitor());

        // 危险的控制点默认禁用, 回读点不受影响
        for key in ["ctlGAPC1_SPCSO2_cmd", "ctlGAPC1_SPCSO3_cmd", "setGAPC3_StrVal7_cmd"] {
            assert!(!by_key(key).enable, "{key} 应默认禁用");
        }
        assert!(by_key("ctlGAPC1_SPCSO3").enable);
        // 每个控制点的回读点都存在(check_feedback 没有把它清掉)
        let controls: Vec<_> = configs.points.iter().filter(|p| p.kind.is_control()).collect();
        assert_eq!(controls.len(), 47);
        assert!(controls.iter().all(|c| c.feedback.is_some()));

        assert_eq!(configs.sources.len(), 3);
        let urcb = &configs.sources[1];
        assert_eq!((urcb.kind, urcb.reference.as_str()), (SourceKind::Urcb, "MONT/LLN0.RP.urcbAin*"));
        assert_eq!(urcb.trg_ops, Some(TRG_DATA_CHANGED | TRG_INTEGRITY));
        assert_eq!(urcb.intg_pd_ms, Some(5000));
        assert_eq!(configs.sources[0].kind, SourceKind::Brcb);
        assert!(!configs.sources[0].purge, "默认补发断线期间的事件");
    }

    #[test]
    fn parse_workbook_with_quality_points() {
        // `--with-quality` 生成的测试夹具: 品质/时标点默认禁用
        let configs = build_configs(manifest("collector-iec61850/tests/data/pcs_points.xlsx")).unwrap();
        let of = |k: PointKind| configs.points.iter().filter(|p| p.kind == k).collect::<Vec<_>>();
        assert_eq!((of(PointKind::Yq).len(), of(PointKind::Ytime).len()), (103, 103));
        assert!(of(PointKind::Yq).iter().chain(&of(PointKind::Ytime)).all(|p| !p.enable));
        let q = configs.points.iter().find(|p| p.key == "staGGIO1_Ind1_q").unwrap();
        assert_eq!(q.reference.to_dotted(), "MONT/staGGIO1.Ind1.q");
        assert_eq!(q.reference.fc, Some(Fc::St));
    }

    /// 一个监视点的行: id key name ref fc type
    fn monitor_row(id: f64, key: &str, reference: &str, fc: Data, kind: &str) -> Vec<Data> {
        row(&[
            Float(id),
            S(key.into()),
            Empty,
            S(reference.into()),
            fc,
            S(kind.into()),
        ])
    }

    #[test]
    fn monitor_point_needs_fc_and_leaf() {
        let ok = Iec61850Config::build(&monitor_row(1.0, "k", "MONT/MMXU1.TotW.mag.f", S("MX".into()), "yc"))
            .unwrap();
        assert_eq!(ok.reference.fc, Some(Fc::Mx));
        assert_eq!(ok.reference.to_dotted(), "MONT/MMXU1.TotW.mag.f");
        assert!(ok.enable);
        assert_eq!((ok.scale, ok.offset), (1.0, 0.0));

        // 缺 FC
        assert!(Iec61850Config::build(&monitor_row(1.0, "k", "MONT/MMXU1.TotW.mag.f", Empty, "yc")).is_err());
        // FC 写在 ref 里也行, 但与 fc 列冲突要报错
        let in_ref = Iec61850Config::build(&monitor_row(1.0, "k", "MONT/MMXU1$MX$TotW$mag$f", Empty, "yc")).unwrap();
        assert_eq!(in_ref.reference.fc, Some(Fc::Mx));
        assert!(Iec61850Config::build(&monitor_row(1.0, "k", "MONT/MMXU1$MX$TotW$mag$f", S("ST".into()), "yc")).is_err());
        // 只到 DO 的监视点不是叶子
        assert!(Iec61850Config::build(&monitor_row(1.0, "k", "MONT/MMXU1.TotW", S("MX".into()), "yc")).is_err());
    }

    #[test]
    fn control_point_ignores_fc_and_parses_options() {
        let mut r = monitor_row(1.0, "cmd", "MONT/ctlGAPC1.SPCSO1", S("CO".into()), "yk");
        r.extend([Empty, Empty, Empty, Empty, Empty, Empty, Empty, Empty]); // 6..=13
        r.push(S("interlock, Synchro".into()));
        r.push(S("fb".into()));
        let c = Iec61850Config::build(&r).unwrap();
        assert_eq!(c.reference.fc, None);
        assert_eq!(c.reference.to_dotted(), "MONT/ctlGAPC1.SPCSO1");
        assert_eq!(c.ctl_opts, CtlOpts { interlock: true, synchro: true, test: false });
        assert_eq!(c.feedback, Some("fb"));

        r[14] = S("bogus".into());
        assert!(Iec61850Config::build(&r).is_err());
    }

    #[test]
    fn rejects_bad_rows() {
        // 保留 id、未知类型、scale 为 0、ref 格式错
        assert!(Iec61850Config::build(&monitor_row(65535.0, "k", "L/N.A.b", S("ST".into()), "yx")).is_err());
        assert!(Iec61850Config::build(&monitor_row(1.0, "k", "L/N.A.b", S("ST".into()), "bogus")).is_err());
        assert!(Iec61850Config::build(&monitor_row(1.0, "k", "noslash", S("ST".into()), "yx")).is_err());
        let mut r = monitor_row(1.0, "k", "L/N.A.b", S("MX".into()), "yc");
        r.extend([Float(0.0)]);
        assert!(Iec61850Config::build(&r).is_err());
    }

    #[test]
    fn duplicate_checks() {
        let p = |id, key, reference: &str, fc: &str, kind| {
            Iec61850Config::build(&monitor_row(id, key, reference, S(fc.into()), kind)).unwrap()
        };
        let existing = [p(1.0, "a", "L/N.A.b", "ST", "yx")];
        assert!(check_duplicate(&existing, &p(1.0, "z", "L/N.A.c", "ST", "yx")).is_some()); // id
        assert!(check_duplicate(&existing, &p(2.0, "a", "L/N.A.c", "ST", "yx")).is_some()); // key
        assert!(check_duplicate(&existing, &p(2.0, "z", "L/N.A.b", "ST", "yx")).is_some()); // 引用
        assert!(check_duplicate(&existing, &p(2.0, "z", "L/N.A.b", "MX", "yc")).is_none()); // 同引用不同 FC 可以
        assert!(check_duplicate(&existing, &p(2.0, "z", "L/N.A.q", "ST", "yq")).is_none());
    }

    #[test]
    fn source_rows() {
        let mk = |cells: &[Data]| Iec61850Source::build(cells);
        let s = mk(&[
            S("urcb".into()), S("MONT/LLN0.RP.urcbAin*".into()), Empty, S("dchg,period".into()),
            Float(5000.0), Empty, Empty, Float(1.0), Empty, Float(1.0),
        ])
        .unwrap();
        assert_eq!(s.kind, SourceKind::Urcb);
        assert_eq!(s.trg_ops, Some(TRG_DATA_CHANGED | TRG_INTEGRITY));
        assert_eq!((s.intg_pd_ms, s.buf_tm_ms, s.interval_ms), (Some(5000), None, None));
        assert!(s.gi && !s.purge && s.enable);

        // `$` 写法、purge、interval=0(不重复)
        let b = mk(&[
            S("brcb".into()), S("MONT/LLN0$BR$brcbDin*".into()), Empty, Empty,
            Empty, Empty, Float(0.0), Float(0.0), Float(1.0), Empty,
        ])
        .unwrap();
        assert_eq!((b.interval_ms, b.gi, b.purge, b.enable, b.trg_ops), (Some(0), false, true, true, None));

        // kind 与 ref 不一致、缺 '/'、未知触发条件
        assert!(mk(&[S("urcb".into()), S("MONT/LLN0.BR.x".into())]).is_err());
        assert!(mk(&[S("brcb".into()), S("MONT/LLN0.RP.x".into())]).is_err());
        assert!(mk(&[S("dataset".into()), S("noslash".into())]).is_err());
        assert!(mk(&[S("urcb".into()), S("L/N.RP.x".into()), Empty, S("dchg,bogus".into())]).is_err());
        assert!(mk(&[S("what".into()), S("L/N.RP.x".into())]).is_err());
    }

    #[test]
    fn feedback_must_point_at_a_monitor_point() {
        let mut mon = Iec61850Config::build(&monitor_row(1.0, "fb", "L/N.A.stVal", S("ST".into()), "yx")).unwrap();
        mon.enable = true;
        let ctl = |key: &str, id: f64, fb: &str| {
            let mut r = monitor_row(id, key, "L/N.A", Empty, "yk");
            r.extend([Empty, Empty, Empty, Empty, Empty, Empty, Empty, Empty, Empty]); // 6..=14
            r.push(S(fb.into()));
            Iec61850Config::build(&r).unwrap()
        };
        let mut points = vec![mon, ctl("c1", 2.0, "fb"), ctl("c2", 3.0, "nope")];
        check_feedback(&mut points);
        assert_eq!(points[1].feedback, Some("fb"));
        assert_eq!(points[2].feedback, None);
    }
}
