//! # 有功功率下发约束（EMU 相关业务策略）
//!
//! `collector_core::dispatch::DispatchInterceptor` 只是核心库暴露的通用扩展点，
//! 具体"允许下发多少有功功率"完全是 EMU 的业务规则，因此策略的定义、组合与注册
//! 都放在这里（collector-engine），核心库的 `DataCenter` 不需要知道 EMU、
//! 充放电许可、点位 2003 这些概念。
//!
//! `dispatch` 下发有功功率时，可能同时存在多条互相独立的限制策略（EMU充放电许可、
//! 防逆流、需量控制……）。这些策略本质上都是在回答同一个问题：“这次允许的有功功率
//! 取值范围是多少？”，而不是“把值改成什么”。
//!
//! 因此这里不用责任链式的“依次改值”，而是让每条策略只产出一个 [`PowerLimit`]
//! （允许的上下限 + 触发原因），最终取所有启用中策略的**交集**，一次性钳位，
//! 这样结果与策略的注册顺序无关，且能明确记录到底是哪条策略生效。

use collector_core::{
    core::point::{DownDataPoint, Val},
    dispatch::{DispatchInterceptor, PointReader},
    field::field_registry,
    runtime::emu::EmuPermission,
};

/// 一条有功功率限制策略。
///
/// 只负责“算出允许范围”，不直接修改下发点，方便与其它策略的结果做交集合并。
pub trait PowerLimitPolicy: Send + Sync {
    /// 策略名称，用于日志中标注最终生效的钳位原因
    fn name(&self) -> &'static str;
    /// 策略当前是否启用
    fn enabled(&self) -> bool;
    /// 计算该策略允许的有功功率取值范围
    fn limit(&self, dev_id: &str, reader: &dyn PointReader) -> PowerLimit;
}

/// 一条策略给出的允许区间，`None` 表示该端不设限
#[derive(Clone, Copy, Debug, Default)]
pub struct PowerLimit {
    min: Option<(f64, &'static str)>,
    max: Option<(f64, &'static str)>,
}

impl PowerLimit {
    /// 不设限
    pub fn none() -> Self {
        Self::default()
    }

    /// 不得超过 `max`
    pub fn at_most(max: f64, reason: &'static str) -> Self {
        Self {
            min: None,
            max: Some((max, reason)),
        }
    }

    /// 不得低于 `min`
    pub fn at_least(min: f64, reason: &'static str) -> Self {
        Self {
            min: Some((min, reason)),
            max: None,
        }
    }

    /// 强制钳死为固定值
    pub fn exact(value: f64, reason: &'static str) -> Self {
        Self {
            min: Some((value, reason)),
            max: Some((value, reason)),
        }
    }

    /// 与另一条限制取交集：下限取更大的、上限取更小的
    pub fn combine(self, other: Self) -> Self {
        let min = match (self.min, other.min) {
            (Some(a), Some(b)) => Some(if a.0 >= b.0 { a } else { b }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        let max = match (self.max, other.max) {
            (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
        Self { min, max }
    }

    /// 将 `value` 钳到区间内，返回钳位后的值以及触发钳位的策略名（未触发则为 `None`）
    pub fn clamp(&self, value: f64) -> (f64, Option<&'static str>) {
        let mut v = value;
        let mut reason = None;

        if let Some((max, name)) = self.max
            && v > max
        {
            v = max;
            reason = Some(name);
        }
        if let Some((min, name)) = self.min
            && v < min
        {
            v = min;
            reason = Some(name);
        }

        (v, reason)
    }
}

/// 汇总多条 [`PowerLimitPolicy`]，在下发前对目标点做统一钳位。
///
/// 目标点位不再是构造时固定的 [`PointRef`]，而是一个 [`FieldRegistry`](collector_core::field::FieldRegistry)
/// 逻辑字段 key：每次拦截都重新 `resolve` 一次，这样管理端在覆盖表里改绑了设备/点位后
/// （无需重启，调用一次 `reload` 即可），这里的钳位目标能立即跟着变，不会钳错点。
pub struct PowerDispatchGuard {
    field_key: &'static str,
    policies: Vec<Box<dyn PowerLimitPolicy>>,
}

impl PowerDispatchGuard {
    pub fn new(field_key: &'static str) -> Self {
        Self {
            field_key,
            policies: Vec::new(),
        }
    }

    pub fn register(&mut self, policy: Box<dyn PowerLimitPolicy>) {
        self.policies.push(policy);
    }
}

impl DispatchInterceptor for PowerDispatchGuard {
    fn name(&self) -> &'static str {
        "有功功率限制"
    }

    /// 找到下发点列表中的目标功率点，用所有启用策略的交集限制钳位它的值
    fn intercept(&self, dev_id: &str, points: &mut [DownDataPoint], reader: &dyn PointReader) {
        let Some(binding) = field_registry().resolve(self.field_key) else {
            return;
        };
        if binding.dev_id != dev_id {
            return;
        }
        let Some(point) = points.iter_mut().find(|it| it.point == binding.point) else {
            return;
        };
        let Ok(value) = point.value.as_f64() else {
            return;
        };

        let limit = self
            .policies
            .iter()
            .filter(|policy| policy.enabled())
            .fold(PowerLimit::none(), |acc, policy| {
                acc.combine(policy.limit(dev_id, reader))
            });

        let (clamped, reason) = limit.clamp(value);
        if let Some(reason) = reason {
            tracing::warn!("[{dev_id}] 有功功率 {value} 被策略[{reason}]钳位到 {clamped}");
            point.value = Val::F64(clamped);
        }
    }
}

/// EMU 充放电许可对有功功率下发的限制策略
///
/// 许可状态存放在虚拟设备 "emu" 的 2 号点位，与具体被下发的设备无关，
/// 因此读取时固定读 "emu"/2，忽略传入的 `dev_id`。
///
/// 只有在 EMU 功能启用时（[`crate::emu::core::Emu::new`] 被调用）才会被
/// 构建并注册进 `DataCenter`，未启用 EMU 的部署完全不会触碰有功功率钳位逻辑。
pub struct EmuPolicy;

impl PowerLimitPolicy for EmuPolicy {
    fn name(&self) -> &'static str {
        "EMU充放电许可"
    }

    fn enabled(&self) -> bool {
        true
    }

    fn limit(&self, _dev_id: &str, reader: &dyn PointReader) -> PowerLimit {
        let permission = reader
            .read("emu", 2)
            .and_then(|it| EmuPermission::try_from(it.value.as_u32().unwrap_or(3) as u8).ok())
            .unwrap_or(EmuPermission::TotalStop);

        match permission {
            EmuPermission::Normal => PowerLimit::none(),
            EmuPermission::ChargeDisabled => PowerLimit::at_most(0.0, "EMU禁充"),
            EmuPermission::DischargeDisabled => PowerLimit::at_least(0.0, "EMU禁放"),
            EmuPermission::TotalStop => PowerLimit::exact(0.0, "EMU禁充禁放"),
        }
    }
}
