use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use collector_iec61850::{Connection, ControlOptions, Error};
use tracing::{info, warn};

use super::convert::add_cause_text;
use super::session::Session;
use crate::config::iec61850_conf::{Iec61850Config, Iec61850Configs, PointKind};
use crate::core::point::{PointId, PointRef, Val};

/// 命令应答成功后等待回读点变化的最长时间。
pub(super) const CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);
const CONFIRM_STEP: Duration = Duration::from_millis(100);

/// 控制点索引，支持按 ID / Key / Name 解析下发目标。
pub(super) struct ControlIndex {
    by_id: HashMap<PointId, Iec61850Config>,
    by_key: HashMap<Arc<str>, PointId>,
    by_name: HashMap<Arc<str>, PointId>,
    /// 启用的监视点, 按 key 查回读点
    monitors: HashMap<Arc<str>, Iec61850Config>,
}

impl ControlIndex {
    pub(super) fn build(configs: &Iec61850Configs) -> Self {
        let mut index = Self {
            by_id: HashMap::new(),
            by_key: HashMap::new(),
            by_name: HashMap::new(),
            monitors: HashMap::new(),
        };
        for cfg in configs.points.iter().filter(|c| c.enable) {
            if cfg.kind.is_control() {
                index.by_id.insert(cfg.id, cfg.clone());
                index.by_key.insert(cfg.key.clone(), cfg.id);
                if !cfg.name.is_empty() {
                    index.by_name.insert(cfg.name.clone(), cfg.id);
                }
            } else {
                index.monitors.insert(cfg.key.clone(), cfg.clone());
            }
        }
        index
    }

    pub(super) fn resolve(&self, point: &PointRef) -> Option<&Iec61850Config> {
        let id = match point {
            PointRef::Id(id) => *id,
            PointRef::Key(key) => *self.by_key.get(key.as_str())?,
            PointRef::Name(name) => *self.by_name.get(name.as_str())?,
        };
        self.by_id.get(&id)
    }

    /// 控制点配置的回读点(必须是启用的监视点)。
    pub(super) fn feedback_of(&self, cfg: &Iec61850Config) -> Option<&Iec61850Config> {
        self.monitors.get(&cfg.feedback.clone()?)
    }
}

/// 下发值 -> 发给 IED 的 `ctlVal`(遥控 1/0, 遥调按 scale/offset 还原)。
pub(super) fn command_value(cfg: &Iec61850Config, value: &Val) -> anyhow::Result<f64> {
    match cfg.kind {
        PointKind::Yk => {
            let on = value.as_bool().map_err(|_| anyhow!("不支持的下发值类型"))?;
            Ok(f64::from(u8::from(on)))
        }
        PointKind::Yt => {
            let v = value.as_f64().map_err(|_| anyhow!("不支持的下发值类型"))?;
            let raw = (v - cfg.offset) / cfg.scale;
            if raw.is_finite() {
                Ok(raw)
            } else {
                Err(anyhow!("下发值不是有限数: {v}"))
            }
        }
        _ => Err(anyhow!("监视点不支持下发")),
    }
}

/// 下发一条命令(阻塞)。控制模型(直接/SBO)由库根据 IED 的 ctlModel 自动选择。
pub(super) fn execute(conn: &Connection, cfg: &Iec61850Config, raw: f64) -> Result<(), Error> {
    let target = conn.resolve_ref(&cfg.reference)?.to_dotted();
    conn.control(
        &target,
        raw,
        &ControlOptions {
            test: cfg.ctl_opts.test,
            interlock_check: cfg.ctl_opts.interlock,
            synchro_check: cfg.ctl_opts.synchro,
            oper_time_ms: 0,
        },
    )
}

/// 控制失败的错误描述: 把 IED 返回的 AddCause 翻译成文字。
pub(super) fn describe(err: &Error) -> String {
    match err {
        Error::Control { failure, .. } => format!(
            "{err}: {}",
            if failure.add_cause == 0 && failure.error == 0 {
                "设备没有给出原因"
            } else {
                add_cause_text(failure.add_cause)
            }
        ),
        other => other.to_string(),
    }
}

/// 下发后读回确认所需的信息。
pub(super) struct Verification {
    pub(super) dev_id: String,
    pub(super) control_key: Arc<str>,
    pub(super) feedback: Iec61850Config,
    /// 下发前回读点的值; 读不到时为 `None`
    pub(super) before: Option<Val>,
    /// 命令成功执行后回读点应有的值
    pub(super) expected: Val,
}

impl Verification {
    pub(super) fn new(
        dev_id: &str,
        control: &Iec61850Config,
        feedback: &Iec61850Config,
        before: Option<Val>,
        value: &Val,
    ) -> Option<Self> {
        let expected = match control.kind {
            PointKind::Yk => Val::U8(u8::from(value.as_bool().ok()?)),
            _ => Val::F64(value.as_f64().ok()?),
        };
        Some(Self {
            dev_id: dev_id.to_string(),
            control_key: control.key.clone(),
            feedback: feedback.clone(),
            before,
            expected,
        })
    }

    /// 回读值是否说明命令已生效: 等于目标值, 或者相对下发前发生了变化。
    ///
    /// 回读点的编码不一定和命令一致(比如遥控用布尔、回读是双点位置), 所以"变化"也算数;
    /// 但如果设备根本没动、回读值一直不变, 就只能判定为未确认。
    pub(super) fn is_confirmed(&self, current: &Val) -> bool {
        let equal = match (&self.expected, current) {
            (Val::F64(a), Val::F64(b)) => (a - b).abs() <= 1e-5 * a.abs().max(1.0),
            (a, b) => a == b,
        };
        equal || self.before.as_ref().is_some_and(|b| b != current)
    }
}

/// 命令应答成功后, 轮询回读点直到确认或超时(阻塞, 放在独立的阻塞任务里跑)。
///
/// 61850 的 direct-normal 控制在服务端检查通过后就回成功, 真正执行是异步的, 客户端无法
/// 从应答得知设备是否执行, 所以只能看回读点。未确认只告警, 不影响后续命令。
pub(super) fn confirm(session: &Session, v: &Verification) {
    let start = Instant::now();
    loop {
        if let Some(current) = session.read_point(&v.feedback)
            && v.is_confirmed(&current)
        {
            info!(
                "[{}] 命令 {} 已生效: 回读 {} = {} ({}ms)",
                v.dev_id,
                v.control_key,
                v.feedback.key,
                current,
                start.elapsed().as_millis()
            );
            return;
        }
        if start.elapsed() >= CONFIRM_TIMEOUT {
            warn!(
                "[{}] 命令 {} 应答成功, 但回读点 {} 在 {}s 内没有变化, 设备可能没有执行",
                v.dev_id,
                v.control_key,
                v.feedback.key,
                CONFIRM_TIMEOUT.as_secs()
            );
            return;
        }
        std::thread::sleep(CONFIRM_STEP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::iec61850_conf::CtlOpts;
    use collector_iec61850::{Fc, ObjectRef};

    fn cfg(kind: PointKind, scale: f64, offset: f64) -> Iec61850Config {
        Iec61850Config {
            id: 1,
            key: "k".into(),
            name: "n".into(),
            reference: "L/N.A.b".parse::<ObjectRef>().unwrap().with_fc(Fc::St),
            kind,
            scale,
            offset,
            unit: None,
            enable: true,
            trans: None,
            status_words: None,
            warn_bits: None,
            level: None,
            ctl_opts: CtlOpts::default(),
            feedback: None,
        }
    }

    fn named(
        id: u32,
        key: &'static str,
        name: &'static str,
        kind: PointKind,
        enable: bool,
        fb: Option<&'static str>,
    ) -> Iec61850Config {
        Iec61850Config {
            id,
            key: key.into(),
            name: name.into(),
            enable,
            feedback: fb.map(Arc::from),
            ..cfg(kind, 1.0, 0.0)
        }
    }

    #[test]
    fn control_index_only_exposes_enabled_control_points() {
        let configs = Iec61850Configs {
            points: vec![
                named(1, "state", "状态", PointKind::Yx, true, None),
                named(2, "stateOff", "停用的回读", PointKind::Yx, false, None),
                named(3, "cmd", "合闸", PointKind::Yk, true, Some("state")),
                named(4, "cmdOff", "复位", PointKind::Yk, false, Some("state")),
                named(
                    5,
                    "cmdNoFb",
                    "无回读",
                    PointKind::Yt,
                    true,
                    Some("stateOff"),
                ),
            ],
            sources: vec![],
        };
        let index = ControlIndex::build(&configs);
        // 按 id / key / name 三种引用都能找到启用的控制点
        for point in [
            PointRef::Id(3),
            PointRef::Key("cmd".into()),
            PointRef::Name("合闸".into()),
        ] {
            assert_eq!(index.resolve(&point).map(|c| c.id), Some(3), "{point:?}");
        }
        // 禁用的控制点、监视点、不存在的点都找不到
        for point in [
            PointRef::Id(4),
            PointRef::Key("cmdOff".into()),
            PointRef::Name("复位".into()),
            PointRef::Id(1),
            PointRef::Id(99),
        ] {
            assert!(index.resolve(&point).is_none(), "{point:?}");
        }
        // 回读点必须是启用的监视点
        assert_eq!(
            index
                .feedback_of(index.resolve(&PointRef::Id(3)).unwrap())
                .map(|c| c.key.clone()),
            Some("state".into())
        );
        assert!(
            index
                .feedback_of(index.resolve(&PointRef::Id(5)).unwrap())
                .is_none(),
            "回读点被禁用 -> 无法确认"
        );
    }

    #[test]
    fn command_values() {
        let yk = cfg(PointKind::Yk, 1.0, 0.0);
        assert_eq!(command_value(&yk, &Val::U8(1)).unwrap(), 1.0);
        assert_eq!(command_value(&yk, &Val::U8(0)).unwrap(), 0.0);
        assert_eq!(command_value(&yk, &Val::F64(0.5)).unwrap(), 1.0);

        let yt = cfg(PointKind::Yt, 0.5, 10.0);
        assert_eq!(command_value(&yt, &Val::F64(60.0)).unwrap(), 100.0); // (60-10)/0.5
        assert!(command_value(&yt, &Val::List(vec![])).is_err());
        assert!(command_value(&cfg(PointKind::Yc, 1.0, 0.0), &Val::U8(1)).is_err());
    }

    fn verification(before: Option<Val>, expected: Val) -> Verification {
        Verification {
            dev_id: "d".into(),
            control_key: "c".into(),
            feedback: cfg(PointKind::Yx, 1.0, 0.0),
            before,
            expected,
        }
    }

    #[test]
    fn confirmed_when_equal_or_changed() {
        // 遥控: 目标 1
        let v = verification(Some(Val::U8(0)), Val::U8(1));
        assert!(v.is_confirmed(&Val::U8(1)), "等于目标值");
        // 回读是双点位置(合=2), 与命令编码不同, 但相对下发前变了
        assert!(v.is_confirmed(&Val::U8(2)), "相对下发前变化");
        assert!(!v.is_confirmed(&Val::U8(0)), "没变且不等于目标值 -> 未确认");

        // 下发前读不到: 只能看是否等于目标值
        let v = verification(None, Val::U8(1));
        assert!(v.is_confirmed(&Val::U8(1)));
        assert!(!v.is_confirmed(&Val::U8(0)));

        // 重复下发同样的值: 回读本来就等于目标值, 算确认
        let v = verification(Some(Val::U8(1)), Val::U8(1));
        assert!(v.is_confirmed(&Val::U8(1)));
    }

    #[test]
    fn analog_confirmation_tolerates_float_rounding() {
        let v = verification(Some(Val::F64(0.0)), Val::F64(1234.5));
        assert!(v.is_confirmed(&Val::F64(1234.5)));
        assert!(
            v.is_confirmed(&Val::F64(1_234.500_01)),
            "f32 往返的微小误差"
        );
        // 与下发前不同也算变化, 所以这里用没变的值验证未确认
        let v = verification(Some(Val::F64(7.0)), Val::F64(1234.5));
        assert!(!v.is_confirmed(&Val::F64(7.0)));
    }

    #[test]
    fn verification_expected_value_follows_the_control_kind() {
        let fb = cfg(PointKind::Yx, 1.0, 0.0);
        let yk = cfg(PointKind::Yk, 1.0, 0.0);
        let v = Verification::new("d", &yk, &fb, None, &Val::U8(1)).unwrap();
        assert_eq!(v.expected, Val::U8(1));
        let yt = cfg(PointKind::Yt, 1.0, 0.0);
        let v = Verification::new("d", &yt, &fb, None, &Val::F64(2.5)).unwrap();
        assert_eq!(v.expected, Val::F64(2.5));
        assert!(Verification::new("d", &yt, &fb, None, &Val::List(vec![])).is_none());
    }

    #[test]
    fn describe_translates_add_cause() {
        use collector_iec61850::ControlFailure;
        let e = Error::Control {
            client_code: 99,
            failure: ControlFailure {
                error: 1,
                add_cause: 10,
                ctl_num: 0,
            },
        };
        assert!(describe(&e).ends_with("被联锁闭锁"), "{}", describe(&e));
        let e = Error::Control {
            client_code: 99,
            failure: ControlFailure {
                error: 0,
                add_cause: 0,
                ctl_num: 0,
            },
        };
        assert!(describe(&e).ends_with("设备没有给出原因"));
        assert_eq!(
            describe(&Error::CreateFailed),
            Error::CreateFailed.to_string()
        );
    }
}
