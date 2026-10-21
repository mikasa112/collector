//! 北向 IEC 104 子站（服务端）点表：把 DataCenter 里的点以 (CA, IOA) 暴露给上级主站。
//!
//! 只读取名为 `iec104_server` 的 sheet，第 1 行为表头。字段说明见 `docs/iec104_server_config.md`。

use std::{collections::HashSet, path::Path};

use calamine::{Data, DataType, HeaderRow, Reader, Xlsx, open_workbook};
use collector_iec60870::{CommandValue, DoublePointValue, PointValue};

use crate::{
    config::{required_f64, required_str, required_usize_integerish},
    core::point::{PointId, Val},
};

pub(crate) const SHEET_NAME: &str = "iec104_server";

/// 点类型：遥信/遥测为上送（监视方向），遥控/遥调为接收上级命令（控制方向）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServerPointKind {
    /// 单点遥信 M_SP_NA_1 / M_SP_TB_1
    Yx,
    /// 双点遥信 M_DP_NA_1 / M_DP_TB_1，值 0 中间/1 分/2 合/3 不确定
    YxDouble,
    /// 短浮点遥测 M_ME_NC_1 / M_ME_TF_1
    YcShort,
    /// 标度化遥测 M_ME_NB_1 / M_ME_TE_1
    YcScaled,
    /// 归一化遥测 M_ME_NA_1 / M_ME_TD_1
    YcNormalized,
    /// 单点遥控 C_SC_NA_1
    Yk,
    /// 双点遥控 C_DC_NA_1，值 1 分/2 合
    YkDouble,
    /// 短浮点遥调 C_SE_NC_1
    YtShort,
    /// 标度化遥调 C_SE_NB_1
    YtScaled,
    /// 归一化遥调 C_SE_NA_1
    YtNormalized,
}

impl ServerPointKind {
    pub(crate) fn is_monitor(self) -> bool {
        matches!(
            self,
            Self::Yx | Self::YxDouble | Self::YcShort | Self::YcScaled | Self::YcNormalized
        )
    }
}

impl TryFrom<&str> for ServerPointKind {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "yx" => Ok(Self::Yx),
            "yx_double" => Ok(Self::YxDouble),
            "yc_short" => Ok(Self::YcShort),
            "yc_scaled" => Ok(Self::YcScaled),
            "yc_normalized" => Ok(Self::YcNormalized),
            "yk" => Ok(Self::Yk),
            "yk_double" => Ok(Self::YkDouble),
            "yt_short" => Ok(Self::YtShort),
            "yt_scaled" => Ok(Self::YtScaled),
            "yt_normalized" => Ok(Self::YtNormalized),
            other => Err(anyhow::anyhow!("未知的点类型: {other}")),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NorthIec104Config {
    pub(crate) ca: i32,
    pub(crate) ioa: i32,
    pub(crate) kind: ServerPointKind,
    /// 数据来源 DataCenter 的设备 id，如 `emu`
    pub(crate) source: String,
    pub(crate) point_id: PointId,
    /// 上送值 = (工程值 - offset) / scale，与 IEC104 主站点表的 `工程值 = 原始值 * scale + offset` 互逆
    pub(crate) scale: f64,
    pub(crate) offset: f64,
    /// 遥测死区（工程值绝对量），变化未超过死区不上送；0 表示有变化就上送
    pub(crate) deadband: f64,
    /// 上送时是否带 CP56Time2a 时标
    pub(crate) timestamp: bool,
    pub(crate) name: String,
}

impl NorthIec104Config {
    fn build(row: &[Data]) -> Result<Option<Self>, anyhow::Error> {
        let ca = i32::try_from(required_usize_integerish(row, 0, "CA")?)
            .map_err(|_| anyhow::anyhow!("CA超出范围"))?;
        let ioa = i32::try_from(required_usize_integerish(row, 1, "IOA")?)
            .map_err(|_| anyhow::anyhow!("IOA超出范围"))?;
        if !(1..=65534).contains(&ca) {
            return Err(anyhow::anyhow!("CA需在1~65534之间: {ca}"));
        }
        if !(1..=0xFF_FFFF).contains(&ioa) {
            return Err(anyhow::anyhow!("IOA需在1~16777215之间: {ioa}"));
        }
        let kind = ServerPointKind::try_from(required_str(row, 2, "TYPE")?)?;
        let source = required_str(row, 3, "来源")?.trim().to_string();
        let point_id = required_f64(row, 4, "点位")? as PointId;
        let scale = row.get(5).and_then(|c| c.as_f64()).unwrap_or(1.0);
        if scale == 0.0 {
            return Err(anyhow::anyhow!("SCALE不能为0"));
        }
        let offset = row.get(6).and_then(|c| c.as_f64()).unwrap_or(0.0);
        let deadband = row.get(7).and_then(|c| c.as_f64()).unwrap_or(0.0);
        if deadband < 0.0 {
            return Err(anyhow::anyhow!("DEADBAND不能为负"));
        }
        let timestamp = row.get(8).and_then(|c| c.as_f64()).unwrap_or(0.0) != 0.0;
        let enable = row.get(9).and_then(|c| c.as_f64()).unwrap_or(1.0) != 0.0;
        let name = row
            .get(10)
            .and_then(|c| c.get_string())
            .unwrap_or_default()
            .to_string();
        if !enable {
            return Ok(None);
        }
        Ok(Some(Self {
            ca,
            ioa,
            kind,
            source,
            point_id,
            scale,
            offset,
            deadband,
            timestamp,
            name,
        }))
    }

    /// 工程值 -> 上送值；类型不匹配或值非法（NaN、超范围）时返回 `None`。
    pub(crate) fn encode(&self, val: &Val) -> Option<PointValue> {
        let raw = |val: &Val| -> Option<f64> {
            let v = (val.as_f64().ok()? - self.offset) / self.scale;
            v.is_finite().then_some(v)
        };
        match self.kind {
            ServerPointKind::Yx => Some(PointValue::Single(val.as_bool().ok()?)),
            ServerPointKind::YxDouble => Some(PointValue::Double(match val.as_u32().ok()? {
                0 => DoublePointValue::Intermediate,
                1 => DoublePointValue::Off,
                2 => DoublePointValue::On,
                3 => DoublePointValue::Indeterminate,
                _ => return None,
            })),
            ServerPointKind::YcShort => Some(PointValue::Short(raw(val)? as f32)),
            ServerPointKind::YcScaled => Some(PointValue::Scaled(
                raw(val)?
                    .round()
                    .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i32,
            )),
            ServerPointKind::YcNormalized => {
                Some(PointValue::Normalized(raw(val)?.clamp(-1.0, 1.0) as f32))
            }
            _ => None,
        }
    }

    /// 监视点默认值（尚未收到数据时，连同"无效"品质一起放进总召）。
    pub(crate) fn default_value(&self) -> Option<PointValue> {
        match self.kind {
            ServerPointKind::Yx => Some(PointValue::Single(false)),
            ServerPointKind::YxDouble => Some(PointValue::Double(DoublePointValue::Intermediate)),
            ServerPointKind::YcShort => Some(PointValue::Short(0.0)),
            ServerPointKind::YcScaled => Some(PointValue::Scaled(0)),
            ServerPointKind::YcNormalized => Some(PointValue::Normalized(0.0)),
            _ => None,
        }
    }

    /// 上级下发的命令值 -> DataCenter 下发值；命令类型与点类型不符、值非法时返回 `None`。
    pub(crate) fn decode_command(&self, cmd: &CommandValue) -> Option<Val> {
        let eng = |raw: f64| Val::F64(raw * self.scale + self.offset);
        match (self.kind, cmd) {
            (ServerPointKind::Yk, CommandValue::Single(on)) => Some(Val::U8(u8::from(*on))),
            (ServerPointKind::YkDouble, CommandValue::Double(v)) => match v {
                DoublePointValue::Off => Some(Val::U8(1)),
                DoublePointValue::On => Some(Val::U8(2)),
                _ => None,
            },
            (ServerPointKind::YtShort, CommandValue::Short(v)) => Some(eng(f32_to_f64(*v))),
            (ServerPointKind::YtNormalized, CommandValue::Normalized(v)) => {
                Some(eng(f32_to_f64(*v)))
            }
            (ServerPointKind::YtScaled, CommandValue::Scaled(v)) => Some(eng(f64::from(*v))),
            _ => None,
        }
    }
}

/// 先按最短十进制表示还原再换算，避免 `0.1f32` 之类变成 `0.10000000149`。
fn f32_to_f64(v: f32) -> f64 {
    v.to_string().parse().unwrap_or_else(|_| f64::from(v))
}

#[derive(thiserror::Error, Debug)]
pub enum NorthIec104ConfigsError {
    #[error("Failed to open workbook: {0}")]
    OpenWorkbookError(#[from] calamine::XlsxError),
}

pub(crate) type NorthIec104Configs = Vec<NorthIec104Config>;

pub(crate) fn build_configs<P: AsRef<Path>>(
    path: P,
) -> Result<NorthIec104Configs, NorthIec104ConfigsError> {
    let mut workbook: Xlsx<_> = open_workbook(path)?;
    let sheet = workbook
        .with_header_row(HeaderRow::Row(1))
        .worksheet_range(SHEET_NAME)?;
    let rows: Vec<&[Data]> = sheet.rows().collect();
    Ok(build_from_rows(&rows))
}

fn build_from_rows(rows: &[&[Data]]) -> NorthIec104Configs {
    let mut configs = Vec::new();
    // 监视点、控制点各自在 (CA, IOA) 上唯一；两者可共用同一地址（遥控后回读）
    let mut monitor_addrs = HashSet::new();
    let mut control_addrs = HashSet::new();
    for row in rows {
        match NorthIec104Config::build(row) {
            Ok(Some(cfg)) => {
                let addrs = if cfg.kind.is_monitor() {
                    &mut monitor_addrs
                } else {
                    &mut control_addrs
                };
                if addrs.insert((cfg.ca, cfg.ioa)) {
                    configs.push(cfg);
                } else {
                    tracing::error!("忽略北向IEC104点 CA={} IOA={}: 地址重复", cfg.ca, cfg.ioa);
                }
            }
            Ok(None) => {}
            Err(err) => tracing::error!("构建北向IEC104配置失败: {}", err),
        }
    }
    configs
}

#[cfg(test)]
mod tests {
    use super::*;
    use Data::{Float, String as S};

    fn row(ca: f64, ioa: f64, kind: &str, scale: f64) -> Vec<Data> {
        vec![
            Float(ca),
            Float(ioa),
            S(kind.into()),
            S("emu".into()),
            Float(7.0),
            Float(scale),
            Float(0.0),
            Float(0.5),
            Float(1.0),
        ]
    }

    fn one(kind: &str, scale: f64) -> NorthIec104Config {
        NorthIec104Config::build(&row(1.0, 100.0, kind, scale))
            .unwrap()
            .unwrap()
    }

    #[test]
    fn parse_sample_workbook() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../config/北向/IEC104_SERVER.xlsx"
        );
        let configs = build_configs(path).unwrap();
        assert_eq!(configs.len(), 7);
        assert_eq!(configs.iter().filter(|c| c.kind.is_monitor()).count(), 5);
        let yc = configs.iter().find(|c| c.ioa == 16385).unwrap();
        assert_eq!((yc.ca, yc.source.as_str(), yc.point_id), (1, "bms-1", 2001));
        assert_eq!((yc.scale, yc.deadband), (0.1, 1.0));
        assert!(configs.iter().find(|c| c.ioa == 1).unwrap().timestamp);
    }

    #[test]
    fn parse_row() {
        let c = one("YC_Short", 0.1);
        assert_eq!((c.ca, c.ioa, c.point_id), (1, 100, 7));
        assert_eq!(c.kind, ServerPointKind::YcShort);
        assert_eq!(c.source, "emu");
        assert!(c.timestamp);
        assert_eq!(c.deadband, 0.5);
    }

    #[test]
    fn invalid_rows() {
        assert!(NorthIec104Config::build(&row(0.0, 1.0, "yx", 1.0)).is_err());
        assert!(NorthIec104Config::build(&row(1.0, 0.0, "yx", 1.0)).is_err());
        assert!(NorthIec104Config::build(&row(1.0, 1.0, "bogus", 1.0)).is_err());
        assert!(NorthIec104Config::build(&row(1.0, 1.0, "yx", 0.0)).is_err());
    }

    #[test]
    fn disabled_row_skipped() {
        let mut r = row(1.0, 1.0, "yx", 1.0);
        r.push(Float(0.0));
        assert!(NorthIec104Config::build(&r).unwrap().is_none());
    }

    #[test]
    fn duplicate_addresses() {
        let rows = [
            row(1.0, 1.0, "yx", 1.0),
            row(1.0, 1.0, "yc_short", 1.0), // 监视点地址重复
            row(1.0, 1.0, "yk", 1.0),       // 控制点可与监视点共用
            row(2.0, 1.0, "yx", 1.0),       // 不同 CA 互不冲突
        ];
        let refs: Vec<&[Data]> = rows.iter().map(|r| r.as_slice()).collect();
        assert_eq!(build_from_rows(&refs).len(), 3);
    }

    #[test]
    fn encode_monitor_values() {
        // 工程值 220.0、scale=0.1 -> 上送 2200
        assert_eq!(
            one("yc_scaled", 0.1).encode(&Val::F64(220.0)),
            Some(PointValue::Scaled(2200))
        );
        assert_eq!(
            one("yc_scaled", 1.0).encode(&Val::F64(1e9)),
            Some(PointValue::Scaled(i32::from(i16::MAX)))
        );
        assert_eq!(
            one("yx", 1.0).encode(&Val::U8(1)),
            Some(PointValue::Single(true))
        );
        assert_eq!(
            one("yx_double", 1.0).encode(&Val::U8(2)),
            Some(PointValue::Double(DoublePointValue::On))
        );
        assert_eq!(one("yx_double", 1.0).encode(&Val::U8(4)), None);
        assert_eq!(one("yc_short", 1.0).encode(&Val::F64(f64::NAN)), None);
        assert_eq!(one("yk", 1.0).encode(&Val::U8(1)), None);
    }

    #[test]
    fn decode_commands() {
        assert_eq!(
            one("yk", 1.0).decode_command(&CommandValue::Single(true)),
            Some(Val::U8(1))
        );
        assert_eq!(
            one("yk_double", 1.0).decode_command(&CommandValue::Double(DoublePointValue::On)),
            Some(Val::U8(2))
        );
        assert_eq!(
            one("yk_double", 1.0)
                .decode_command(&CommandValue::Double(DoublePointValue::Indeterminate)),
            None
        );
        // 设点 2200 * 0.1 = 220.0
        assert_eq!(
            one("yt_scaled", 0.1).decode_command(&CommandValue::Scaled(2200)),
            Some(Val::F64(2200.0 * 0.1))
        );
        assert_eq!(
            one("yt_short", 0.1).decode_command(&CommandValue::Short(0.5)),
            Some(Val::F64(0.5 * 0.1))
        );
        // 命令类型与点类型不符
        assert_eq!(
            one("yk", 1.0).decode_command(&CommandValue::Short(1.0)),
            None
        );
    }
}
