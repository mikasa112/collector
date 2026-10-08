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
    core::point::{DownDataPoint, PointRef, Val},
    dispatch::{DispatchInterceptor, PointReader},
    field::field_registry,
    runtime::emu::{EmuPermission, PowerGuardConfig},
};
use parking_lot::Mutex;

/// 关口表有功功率字段（取电为正、反送为负），防逆流与需量保护共用
pub const GRID_POWER_FIELD: &str = "grid_active_power";
/// PCS 实际有功功率字段（点位口径：负充正放，与下发点 2003 的正充负放相反，
/// 策略内部读取时统一取反为正充负放，见 [`read_pcs_power`]）
pub const PCS_POWER_FIELD: &str = "pcs_actual_power";

/// 复评时目标功率变化小于该值（kW）则不重复下发，避免每秒向 PCS 写同一个值
const REDISPATCH_DEADBAND: f64 = 0.5;


/// 一条有功功率限制策略。
///
/// 只负责“算出允许范围”，不直接修改下发点，方便与其它策略的结果做交集合并。
pub trait PowerLimitPolicy: Send + Sync {
    /// 策略名称，用于日志中标注最终生效的钳位原因
    fn name(&self) -> &'static str;
    /// 策略当前是否启用
    fn enabled(&self) -> bool;
    /// 是否为硬约束（设备安全类，如 SOC 许可）。
    /// 硬约束在所有普通策略之后再钳位一次，冲突时永远以硬约束为准
    fn hard(&self) -> bool {
        false
    }
    /// 计算该策略允许的有功功率取值范围。
    /// 有状态的策略（回差）在此推进状态机，同一组输入重复调用结果不变
    fn limit(&self, ctx: &LimitCtx) -> PowerLimit;
}

/// 计算限制时的上下文
pub struct LimitCtx<'a> {
    pub dev_id: &'a str,
    pub reader: &'a dyn PointReader,
    /// 上游（策略/计划曲线/手动）请求的有功功率，钳位前的值，正充负放
    pub requested: f64,
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

    /// 将 `value` 钳到区间内，返回钳位后的值以及触发钳位的策略名（未触发则为 `None`）。
    /// 先钳下限再钳上限，区间矛盾时上限优先
    pub fn clamp(&self, value: f64) -> (f64, Option<&'static str>) {
        let mut v = value;
        let mut reason = None;

        if let Some((min, name)) = self.min
            && v < min
        {
            v = min;
            reason = Some(name);
        }
        if let Some((max, name)) = self.max
            && v > max
        {
            v = max;
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
    last: Mutex<Option<LastDispatch>>,
}

/// 最近一次经过本守卫的下发，供周期复评使用
struct LastDispatch {
    dev_id: String,
    /// 钳位前的上游请求值
    requested: f64,
    /// 实际放行的值
    sent: f64,
}

impl PowerDispatchGuard {
    pub fn new(field_key: &'static str) -> Self {
        Self {
            field_key,
            policies: Vec::new(),
            last: Mutex::new(None),
        }
    }

    pub fn register(&mut self, policy: Box<dyn PowerLimitPolicy>) {
        self.policies.push(policy);
    }

    pub fn field_key(&self) -> &'static str {
        self.field_key
    }

    /// 对 `requested` 求所有启用策略交集后的放行值；普通策略先钳，硬约束最后钳
    pub fn evaluate(
        &self,
        dev_id: &str,
        requested: f64,
        reader: &dyn PointReader,
    ) -> (f64, Option<&'static str>) {
        let ctx = LimitCtx {
            dev_id,
            reader,
            requested,
        };
        let (soft, hard) = self.policies.iter().filter(|policy| policy.enabled()).fold(
            (PowerLimit::none(), PowerLimit::none()),
            |(soft, hard), policy| {
                let limit = policy.limit(&ctx);
                if policy.hard() {
                    (soft, hard.combine(limit))
                } else {
                    (soft.combine(limit), hard)
                }
            },
        );
        let (v, soft_reason) = soft.clamp(requested);
        let (v, hard_reason) = hard.clamp(v);
        (v, hard_reason.or(soft_reason))
    }

    /// 周期复评：用最近一次上游请求值重新求一遍放行值，
    /// 与上次实际放行值相差超过死区时返回需要重新下发的**上游请求值**
    /// （重新走一遍拦截器，保持"记录请求值"的语义）
    pub fn pending_redispatch(&self, reader: &dyn PointReader) -> Option<f64> {
        let (dev_id, requested, sent) = {
            let last = self.last.lock();
            let last = last.as_ref()?;
            (last.dev_id.clone(), last.requested, last.sent)
        };
        let (now, _) = self.evaluate(&dev_id, requested, reader);
        ((now - sent).abs() > REDISPATCH_DEADBAND).then_some(requested)
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

        let (clamped, reason) = self.evaluate(dev_id, value, reader);
        *self.last.lock() = Some(LastDispatch {
            dev_id: dev_id.to_string(),
            requested: value,
            sent: clamped,
        });
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

    fn hard(&self) -> bool {
        true
    }

    fn limit(&self, ctx: &LimitCtx) -> PowerLimit {
        let permission = ctx
            .reader
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

/// 按字段当前绑定读取数值；未绑定、点位不存在、非数值时返回 `None`。
/// 经 [`PointReader`] 读取，因此只支持按 ID 绑定的点位
fn read_field(reader: &dyn PointReader, key: &str) -> Option<f64> {
    let binding = field_registry().resolve(key)?;
    let PointRef::Id(id) = binding.point else {
        return None;
    };
    reader
        .read(&binding.dev_id, id)?
        .value
        .as_f64()
        .ok()
        .filter(|v| v.is_finite())
}

/// 读取 PCS 实际有功功率，取反后与下发口径一致（正充负放）
fn read_pcs_power(reader: &dyn PointReader) -> Option<f64> {
    read_field(reader, PCS_POWER_FIELD).map(|v| -v)
}

#[derive(Default)]
struct HysteresisState {
    active: bool,
    invalid: bool,
}

/// 记录数据有效性的切换，只在变化时打日志，避免每秒刷屏
fn note_validity(state: &mut HysteresisState, valid: bool, name: &str) {
    if state.invalid == !valid {
        return;
    }
    state.invalid = !valid;
    if valid {
        tracing::info!("[{name}] 关口/PCS 功率数据恢复");
    } else {
        tracing::warn!("[{name}] 关口/PCS 功率数据无效");
    }
}

/// 防逆流：限制放电功率，使关口反送不超过阈值。
///
/// 符号：功率正充负放，关口取电为正。下发后关口功率预估为
/// `g_after = P_grid + (P_set - P_pcs)`，`g_free` 为按上游请求值下发时的预估。
/// - 触发：`g_free < -阈值`
/// - 生效期间：目标 `g_after >= -阈值 + 回差`，即 `P_set >= P_pcs - P_grid - 阈值 + 回差`
/// - 释放：`g_free >= -阈值 + 回差`（此时下限不再约束请求值，释放无功率跳变）
///
/// 关口表或 PCS 功率读不到时，保守地禁止放电。
pub struct AntiBackflowPolicy {
    cfg: &'static PowerGuardConfig,
    state: Mutex<HysteresisState>,
}

impl AntiBackflowPolicy {
    pub fn new(cfg: &'static PowerGuardConfig) -> Self {
        Self {
            cfg,
            state: Mutex::new(HysteresisState::default()),
        }
    }
}

impl PowerLimitPolicy for AntiBackflowPolicy {
    fn name(&self) -> &'static str {
        "防逆流"
    }

    fn enabled(&self) -> bool {
        self.cfg.anti_backflow_enable()
    }

    fn limit(&self, ctx: &LimitCtx) -> PowerLimit {
        let mut state = self.state.lock();
        let grid = read_field(ctx.reader, GRID_POWER_FIELD);
        let pcs = read_pcs_power(ctx.reader);
        let (Some(grid), Some(pcs)) = (grid, pcs) else {
            note_validity(&mut state, false, self.name());
            return PowerLimit::at_least(0.0, "防逆流-功率数据无效禁放");
        };
        note_validity(&mut state, true, self.name());

        let threshold = self.cfg.anti_backflow_threshold();
        let hysteresis = self.cfg.anti_backflow_hysteresis();
        let free = grid + (ctx.requested - pcs);

        if !state.active && free < -threshold {
            state.active = true;
            tracing::warn!("[防逆流] 触发: 预估关口功率 {free:.2}kW < -{threshold}kW");
        } else if state.active && free >= -threshold + hysteresis {
            state.active = false;
            tracing::info!("[防逆流] 释放: 预估关口功率 {free:.2}kW");
        }

        if state.active {
            PowerLimit::at_least(pcs - grid - threshold + hysteresis, self.name())
        } else {
            PowerLimit::none()
        }
    }
}

/// 需量保护：限制充电/强制放电，使关口取电不超过需量限制。
///
/// - 触发：`g_free > 需量限制`
/// - 生效期间：目标 `g_after <= 限制 - 回差`，即 `P_set <= P_pcs - P_grid + 限制 - 回差`
/// - 释放：`g_free <= 限制 - 回差`
///
/// 关口表或 PCS 功率读不到时不施加限制（锁死充电的代价更大），只记录日志。
/// 目前按瞬时功率判断，15 分钟滑窗需量预测留作后续扩展。
pub struct DemandGuardPolicy {
    cfg: &'static PowerGuardConfig,
    state: Mutex<HysteresisState>,
}

impl DemandGuardPolicy {
    pub fn new(cfg: &'static PowerGuardConfig) -> Self {
        Self {
            cfg,
            state: Mutex::new(HysteresisState::default()),
        }
    }
}

impl PowerLimitPolicy for DemandGuardPolicy {
    fn name(&self) -> &'static str {
        "需量保护"
    }

    fn enabled(&self) -> bool {
        self.cfg.demand_guard_enable()
    }

    fn limit(&self, ctx: &LimitCtx) -> PowerLimit {
        let mut state = self.state.lock();
        let grid = read_field(ctx.reader, GRID_POWER_FIELD);
        let pcs = read_pcs_power(ctx.reader);
        let (Some(grid), Some(pcs)) = (grid, pcs) else {
            note_validity(&mut state, false, self.name());
            state.active = false;
            return PowerLimit::none();
        };
        note_validity(&mut state, true, self.name());

        let limit = self.cfg.demand_limit();
        let hysteresis = self.cfg.demand_hysteresis();
        let free = grid + (ctx.requested - pcs);

        if !state.active && free > limit {
            state.active = true;
            tracing::warn!("[需量保护] 触发: 预估关口功率 {free:.2}kW > {limit}kW");
        } else if state.active && free <= limit - hysteresis {
            state.active = false;
            tracing::info!("[需量保护] 释放: 预估关口功率 {free:.2}kW");
        }

        if state.active {
            PowerLimit::at_most(pcs - grid + limit - hysteresis, self.name())
        } else {
            PowerLimit::none()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use collector_core::core::point::DataPoint;

    use super::*;
    use crate::emu::FIELDS;

    /// 只存数值的 mock 读取器，键为 (设备, 点位ID)
    #[derive(Default)]
    struct MockReader(HashMap<(String, u32), f64>);

    impl MockReader {
        /// 按字段当前绑定写入数值
        fn set_field(&mut self, key: &str, value: f64) {
            let b = field_registry().resolve(key).unwrap();
            let PointRef::Id(id) = b.point else { panic!() };
            self.0.insert((b.dev_id, id), value);
        }
        fn set_permission(&mut self, p: u8) {
            self.0.insert(("emu".to_string(), 2), p as f64);
        }
    }

    impl PointReader for MockReader {
        fn read(&self, dev_id: &str, point_id: u32) -> Option<DataPoint> {
            let v = *self.0.get(&(dev_id.to_string(), point_id))?;
            Some(DataPoint {
                id: point_id,
                key: "mock",
                name: "mock",
                value: Val::F64(v),
                translator: None,
                bits: None,
                words: None,
                unit: None,
                level: None,
            })
        }
    }

    fn reader(grid: f64, pcs: f64) -> MockReader {
        field_registry().register(FIELDS);
        let mut r = MockReader::default();
        r.set_field(GRID_POWER_FIELD, grid);
        r.set_field(PCS_POWER_FIELD, -pcs); // 点位口径为负充正放
        r
    }

    /// 策略持有 `&'static` 配置（生产中来自全局 runtime），测试里泄漏一份即可
    fn cfg() -> &'static PowerGuardConfig {
        let cfg: &'static PowerGuardConfig = Box::leak(Box::default());
        cfg.set_anti_backflow_enable(true);
        cfg.set_demand_guard_enable(true);
        cfg
    }

    fn eval(policy: &dyn PowerLimitPolicy, r: &MockReader, requested: f64) -> f64 {
        let ctx = LimitCtx {
            dev_id: "pcs",
            reader: r,
            requested,
        };
        policy.limit(&ctx).clamp(requested).0
    }

    #[test]
    fn anti_backflow_triggers_holds_and_releases() {
        let p = AntiBackflowPolicy::new(cfg()); // 阈值0 回差2
        // 未触发：关口取电 5kW，不限制
        assert_eq!(eval(&p, &reader(5.0, -50.0), -50.0), -50.0);
        // 触发：放电 50kW 且反送 10kW，下限 = -50 + 10 - 0 + 2 = -38，调整后关口 = +2
        assert_eq!(eval(&p, &reader(-10.0, -50.0), -50.0), -38.0);
        // 生效中，负荷仍不足以吸收：继续约束，不放开
        assert_eq!(eval(&p, &reader(5.0, -38.0), -50.0), -41.0);
        // 负荷回升，不限制时预估关口 15 + (-50+41) = 6 >= 2，释放并回到请求值
        assert_eq!(eval(&p, &reader(15.0, -41.0), -50.0), -50.0);
        // 释放后不再约束
        assert_eq!(eval(&p, &reader(15.0, -50.0), -50.0), -50.0);
    }

    #[test]
    fn anti_backflow_forbids_discharge_without_data() {
        let p = AntiBackflowPolicy::new(cfg());
        field_registry().register(FIELDS);
        let empty = MockReader::default();
        assert_eq!(eval(&p, &empty, -50.0), 0.0);
        assert_eq!(eval(&p, &empty, 30.0), 30.0); // 充电不受影响
    }

    #[test]
    fn demand_guard_triggers_holds_and_releases() {
        let p = DemandGuardPolicy::new(cfg()); // 限值100 回差5
        // 未触发
        assert_eq!(eval(&p, &reader(90.0, 0.0), 0.0), 0.0);
        // 触发：关口 110kW，上限 = 0 - 110 + 100 - 5 = -15（强制放电），调整后关口 95
        assert_eq!(eval(&p, &reader(110.0, 0.0), 0.0), -15.0);
        // 请求充电 20kW 同样被压到放电
        assert_eq!(eval(&p, &reader(95.0, -15.0), 20.0), -15.0);
        // 负荷下降：预估 60 + (0+30) = 90 <= 95 释放
        assert_eq!(eval(&p, &reader(60.0, -30.0), 0.0), 0.0);
    }

    #[test]
    fn demand_guard_ignores_missing_data() {
        let p = DemandGuardPolicy::new(cfg());
        field_registry().register(FIELDS);
        assert_eq!(eval(&p, &MockReader::default(), 20.0), 20.0);
    }

    #[test]
    fn disabled_policy_is_skipped_by_guard() {
        let mut guard = PowerDispatchGuard::new("pcs_active_power");
        guard.register(Box::new(AntiBackflowPolicy::new(Box::leak(Box::default()))));
        let r = reader(-10.0, -50.0);
        assert_eq!(guard.evaluate("pcs", -50.0, &r), (-50.0, None));
    }

    #[test]
    fn emu_permission_beats_protection() {
        let cfg = cfg();
        let mut guard = PowerDispatchGuard::new("pcs_active_power");
        guard.register(Box::new(EmuPolicy));
        guard.register(Box::new(AntiBackflowPolicy::new(cfg)));
        guard.register(Box::new(DemandGuardPolicy::new(cfg)));

        // 需量保护要求强制放电，但 EMU 禁放（SOC 低）：保持 0
        let mut r = reader(110.0, 0.0);
        r.set_permission(2);
        assert_eq!(guard.evaluate("pcs", 0.0, &r).0, 0.0);

        // 防逆流要求强制充电，但 EMU 禁充（SOC 满）：保持 0
        let mut r = reader(-30.0, 0.0);
        r.set_permission(1);
        assert_eq!(guard.evaluate("pcs", 0.0, &r).0, 0.0);
    }

    #[test]
    fn upper_bound_wins_when_limits_conflict() {
        // 防逆流要求 >= 25，需量保护要求 <= 5：以需量保护（上限）为准
        let lower = PowerLimit::at_least(25.0, "防逆流");
        let upper = PowerLimit::at_most(5.0, "需量保护");
        assert_eq!(lower.combine(upper).clamp(0.0), (5.0, Some("需量保护")));
    }

    #[test]
    fn redispatch_only_when_result_changes() {
        let cfg = cfg();
        let mut guard = PowerDispatchGuard::new("pcs_active_power");
        guard.register(Box::new(AntiBackflowPolicy::new(cfg)));

        // 还没有任何下发记录
        assert_eq!(guard.pending_redispatch(&reader(0.0, 0.0)), None);

        // 模拟一次下发：请求 -50，被钳到 -38
        let r = reader(-10.0, -50.0);
        let (sent, _) = guard.evaluate("pcs", -50.0, &r);
        *guard.last.lock() = Some(LastDispatch {
            dev_id: "pcs".to_string(),
            requested: -50.0,
            sent,
        });
        // 工况未变：无需重发
        assert_eq!(guard.pending_redispatch(&reader(-10.0, -50.0)), None);
        // 负荷回升，限制解除：需要把请求值重新下发
        assert_eq!(guard.pending_redispatch(&reader(15.0, -38.0)), Some(-50.0));
    }
}
