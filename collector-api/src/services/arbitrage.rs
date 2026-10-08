use collector_core::utils::database::get_database;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::{
    dao::{
        electricity::ElectricityDao,
        planned_curve::{PlanCurveDetailDao, PlanCurveMasterDao},
    },
    handlers::arbitrage::ArbitrageParams,
    models::electricity::{ElectricityPeriod, ElectricityPrice, PeriodType},
    services::{
        ServiceError, ServiceResult,
        electricity::{MINUTES_PER_DAY, parse_minutes},
    },
};

/// 一天的 15 分钟时段数，与计划曲线的 time_index(0-95) 对应
const SLOTS: usize = 96;
const SLOT_MINUTES: u32 = 15;
const SLOT_HOURS: f64 = 0.25;
/// 功率输出保留两位小数，对 SOC 轨迹的累计偏差可忽略
const POWER_DECIMALS: f64 = 100.0;

#[derive(Debug, Serialize)]
pub struct ArbitragePoint {
    pub time_index: u8,
    pub time: String,
    pub period_type: PeriodType,
    /// 该时段电价(元/kWh)
    pub price: f64,
    /// 正充负放(kW)
    pub power_value: f64,
    /// 充电时为 soc_max，放电时为 soc_min，空闲为 null；与计划曲线的 soc_limit 语义一致
    pub soc_limit: Option<f64>,
    /// 按模型推算的该时段结束时的 SOC(%)
    pub soc_after: f64,
}

#[derive(Debug, Serialize)]
pub struct ArbitrageSummary {
    /// 电网侧充电电量(kWh)
    pub charge_kwh: f64,
    /// 电网侧放电电量(kWh)
    pub discharge_kwh: f64,
    pub charge_cost: f64,
    pub discharge_income: f64,
    pub degradation_cost: f64,
    /// 日净收益(元) = 放电收入 - 充电成本 - 衰减成本
    pub net_profit: f64,
    pub start_soc: f64,
    pub end_soc: f64,
    /// 当前电价与参数下是否存在套利空间；为 false 时曲线全为 0
    pub has_arbitrage: bool,
}

#[derive(Debug, Serialize)]
pub struct ArbitragePlan {
    pub points: Vec<ArbitragePoint>,
    pub summary: ArbitrageSummary,
}

fn round(v: f64, scale: f64) -> f64 {
    (v * scale).round() / scale
}

fn format_slot(index: usize) -> String {
    let m = index as u32 * SLOT_MINUTES;
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// 将时段表与电价表展开为 96 个时段各自的 (电价类型, 单价)，按时段开始时刻归属
fn slot_prices(
    periods: &[ElectricityPeriod],
    prices: &[ElectricityPrice],
) -> ServiceResult<Vec<(PeriodType, f64)>> {
    if periods.is_empty() {
        return Err(ServiceError::InvalidParameter(
            "尚未配置电价时段，请先配置时段表".to_string(),
        ));
    }
    let ranges: Vec<(u32, u32, PeriodType)> = periods
        .iter()
        .filter_map(|p| {
            Some((
                parse_minutes(&p.start_time)?,
                parse_minutes(&p.end_time)?,
                p.period_type,
            ))
        })
        .collect();
    (0..SLOTS)
        .map(|slot| {
            let minute = slot as u32 * SLOT_MINUTES;
            debug_assert!(minute < MINUTES_PER_DAY);
            let (_, _, period_type) = ranges
                .iter()
                .find(|(s, e, _)| *s <= minute && minute < *e)
                .ok_or_else(|| {
                    ServiceError::InvalidParameter(format!(
                        "电价时段未覆盖{}，请重新配置完整的时段表",
                        format_slot(slot)
                    ))
                })?;
            let price = prices
                .iter()
                .find(|p| p.period_type == *period_type)
                .map(|p| p.price)
                .ok_or_else(|| {
                    ServiceError::InvalidParameter(format!(
                        "尚未配置“{}”的电价",
                        period_type.name()
                    ))
                })?;
            Ok((*period_type, price))
        })
        .collect()
}

/// SOC 离散网格：以 initial_soc 为锚点向两侧按步长展开，保证起点精确落在网格上且不越界
struct Grid {
    start: usize,
    end: usize,
    states: usize,
}

fn build_grid(p: &ArbitrageParams) -> ServiceResult<Grid> {
    if p.soc_min >= p.soc_max {
        return Err(ServiceError::InvalidParameter(
            "soc_min必须小于soc_max".to_string(),
        ));
    }
    let end_soc = p.end_soc.unwrap_or(p.initial_soc);
    for (name, v) in [("initial_soc", p.initial_soc), ("end_soc", end_soc)] {
        if v < p.soc_min || v > p.soc_max {
            return Err(ServiceError::InvalidParameter(format!(
                "{name}必须在soc_min与soc_max之间"
            )));
        }
    }
    let eps = 1e-9;
    let below = ((p.initial_soc - p.soc_min) / p.soc_step + eps).floor() as usize;
    let above = ((p.soc_max - p.initial_soc) / p.soc_step + eps).floor() as usize;
    let states = below + above + 1;
    let offset = (end_soc - p.initial_soc) / p.soc_step;
    let end = (below as f64 + offset)
        .round()
        .clamp(0.0, (states - 1) as f64) as usize;
    Ok(Grid {
        start: below,
        end,
        states,
    })
}

/// 动态规划求每个时段的功率(kW，正充负放)。
/// 状态是离散 SOC，转移时由 SOC 变化量反推电网侧功率，因此功率不需要单独离散。
fn solve_powers(p: &ArbitrageParams, grid: &Grid, prices: &[f64]) -> ServiceResult<Vec<f64>> {
    let n = grid.states;
    let eta = p.round_trip_efficiency.sqrt();
    // 相邻 SOC 状态之间的电池内部能量差(kWh)
    let e_step = p.capacity_kwh * p.soc_step / 100.0;
    let max_up = ((p.max_charge_kw * SLOT_HOURS * eta / e_step).floor() as usize).min(n - 1);
    let max_down = ((p.max_discharge_kw * SLOT_HOURS / eta / e_step).floor() as usize).min(n - 1);
    let deg = p.degradation_cost_per_kwh;

    // 从状态 i 转到 j 的成本；充电花钱，放电为负成本
    let cost = |price: f64, i: usize, j: usize| -> f64 {
        if j > i {
            (price + deg) * (j - i) as f64 * e_step / eta
        } else {
            (deg - price) * (i - j) as f64 * e_step * eta
        }
    };

    let mut cur = vec![f64::INFINITY; n];
    cur[grid.start] = 0.0;
    let mut back = vec![vec![0u16; n]; SLOTS];
    for (t, &price) in prices.iter().enumerate() {
        let mut next = vec![f64::INFINITY; n];
        for j in 0..n {
            // 先取“不动”，成本相同时优先保持空闲，避免出现无意义的抖动
            let mut best = cur[j];
            let mut best_i = j;
            let lo = j.saturating_sub(max_up);
            let hi = (j + max_down).min(n - 1);
            for i in lo..=hi {
                if i == j || cur[i].is_infinite() {
                    continue;
                }
                let c = cur[i] + cost(price, i, j);
                if c < best - 1e-9 {
                    best = c;
                    best_i = i;
                }
            }
            next[j] = best;
            back[t][j] = best_i as u16;
        }
        cur = next;
    }
    if cur[grid.end].is_infinite() {
        return Err(ServiceError::InvalidParameter(
            "按当前功率无法在一天内从initial_soc到达end_soc".to_string(),
        ));
    }

    let mut powers = vec![0.0; SLOTS];
    let mut j = grid.end;
    for t in (0..SLOTS).rev() {
        let i = back[t][j] as usize;
        powers[t] = if j > i {
            (j - i) as f64 * e_step / eta / SLOT_HOURS
        } else {
            -((i - j) as f64 * e_step * eta / SLOT_HOURS)
        };
        j = i;
    }
    Ok(powers
        .into_iter()
        .map(|v| round(v, POWER_DECIMALS))
        .collect())
}

/// 纯计算：给定参数与 96 个时段的电价，生成套利曲线。不依赖数据库，便于单元测试
fn build_plan(p: &ArbitrageParams, slots: &[(PeriodType, f64)]) -> ServiceResult<ArbitragePlan> {
    let grid = build_grid(p)?;
    let prices: Vec<f64> = slots.iter().map(|(_, price)| *price).collect();
    let powers = solve_powers(p, &grid, &prices)?;

    let eta = p.round_trip_efficiency.sqrt();
    let mut soc = p.initial_soc;
    let (mut charge_kwh, mut discharge_kwh) = (0.0, 0.0);
    let (mut charge_cost, mut discharge_income) = (0.0, 0.0);
    let mut points = Vec::with_capacity(SLOTS);
    for (t, (&power, &(period_type, price))) in powers.iter().zip(slots).enumerate() {
        let kwh = power * SLOT_HOURS;
        let stored = if power > 0.0 { kwh * eta } else { kwh / eta };
        soc += stored / p.capacity_kwh * 100.0;
        if power > 0.0 {
            charge_kwh += kwh;
            charge_cost += kwh * price;
        } else if power < 0.0 {
            discharge_kwh += -kwh;
            discharge_income += -kwh * price;
        }
        points.push(ArbitragePoint {
            time_index: t as u8,
            time: format_slot(t),
            period_type,
            price,
            power_value: power,
            soc_limit: if power > 0.0 {
                Some(p.soc_max)
            } else if power < 0.0 {
                Some(p.soc_min)
            } else {
                None
            },
            soc_after: round(soc, 100.0),
        });
    }
    let degradation_cost = p.degradation_cost_per_kwh * (charge_kwh + discharge_kwh);
    Ok(ArbitragePlan {
        summary: ArbitrageSummary {
            charge_kwh: round(charge_kwh, 100.0),
            discharge_kwh: round(discharge_kwh, 100.0),
            charge_cost: round(charge_cost, 100.0),
            discharge_income: round(discharge_income, 100.0),
            degradation_cost: round(degradation_cost, 100.0),
            net_profit: round(discharge_income - charge_cost - degradation_cost, 100.0),
            start_soc: p.initial_soc,
            end_soc: round(soc, 100.0),
            has_arbitrage: powers.iter().any(|v| *v != 0.0),
        },
        points,
    })
}

pub struct ArbitrageService {
    pool: SqlitePool,
}

impl ArbitrageService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            pool: get_database()?,
        })
    }

    /// 按当前电价与时段配置计算套利曲线，不写库
    pub async fn plan(&self, params: ArbitrageParams) -> ServiceResult<ArbitragePlan> {
        let periods = ElectricityDao::find_periods(&self.pool).await?;
        let prices = ElectricityDao::find_prices(&self.pool).await?;
        let slots = slot_prices(&periods, &prices)?;
        // DP 是纯 CPU 计算，放到阻塞线程池避免占住异步运行时
        tokio::task::spawn_blocking(move || build_plan(&params, &slots)).await?
    }

    /// 重新计算后整体覆盖指定计划曲线的 96 个时段。以服务端计算结果为准，不接受客户端传入的曲线
    pub async fn apply(
        &self,
        curve_id: u32,
        params: ArbitrageParams,
    ) -> ServiceResult<ArbitragePlan> {
        if PlanCurveMasterDao::find_by_id(&self.pool, curve_id)
            .await?
            .is_none()
        {
            return Err(ServiceError::NotFound(format!(
                "{curve_id}的计划曲线不存在"
            )));
        }
        let plan = self.plan(params).await?;
        // 全 0 曲线会把原有曲线清空，没有套利空间时拒绝写入
        if !plan.summary.has_arbitrage {
            return Err(ServiceError::BusinessLogic(
                "当前电价与参数下没有套利空间，未写入曲线".to_string(),
            ));
        }
        let details: Vec<(u8, f64, Option<f64>)> = plan
            .points
            .iter()
            .map(|p| (p.time_index, p.power_value, p.soc_limit))
            .collect();
        PlanCurveDetailDao::upsert_details(&self.pool, curve_id, &details).await?;
        Ok(plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> ArbitrageParams {
        ArbitrageParams {
            max_charge_kw: 125.0,
            max_discharge_kw: 125.0,
            capacity_kwh: 261.0,
            soc_min: 10.0,
            soc_max: 90.0,
            initial_soc: 10.0,
            end_soc: None,
            round_trip_efficiency: 0.9,
            degradation_cost_per_kwh: 0.0,
            soc_step: 1.0,
        }
    }

    /// 谷 0-6、22-24，平 6-10，尖 10-12，峰 12-22
    fn tariff(valley: f64, flat: f64, sharp: f64, peak: f64) -> Vec<(PeriodType, f64)> {
        (0..SLOTS)
            .map(|t| match t as u32 * SLOT_MINUTES {
                m if m < 6 * 60 || m >= 22 * 60 => (PeriodType::Valley, valley),
                m if m < 10 * 60 => (PeriodType::Flat, flat),
                m if m < 12 * 60 => (PeriodType::Sharp, sharp),
                _ => (PeriodType::Peak, peak),
            })
            .collect()
    }

    fn period_of(plan: &ArbitragePlan, t: PeriodType) -> impl Iterator<Item = &ArbitragePoint> {
        plan.points.iter().filter(move |p| p.period_type == t)
    }

    #[test]
    fn charges_in_valley_and_discharges_at_high_price() {
        let plan = build_plan(&params(), &tariff(0.3, 0.6, 1.2, 0.9)).unwrap();
        assert!(plan.summary.has_arbitrage);
        assert!(plan.summary.net_profit > 0.0);
        // 只在谷段充电
        assert!(
            plan.points
                .iter()
                .filter(|p| p.power_value > 0.0)
                .all(|p| p.period_type == PeriodType::Valley)
        );
        // 只在尖、峰段放电，且尖段电价最高，应先于峰段放
        assert!(
            plan.points
                .iter()
                .filter(|p| p.power_value < 0.0)
                .all(|p| matches!(p.period_type, PeriodType::Sharp | PeriodType::Peak))
        );
        let sharp: f64 = period_of(&plan, PeriodType::Sharp)
            .map(|p| -p.power_value)
            .sum();
        assert!(sharp > 0.0);
        // 平段不动作
        assert!(period_of(&plan, PeriodType::Flat).all(|p| p.power_value == 0.0));
    }

    #[test]
    fn respects_power_and_soc_bounds_and_returns_to_start() {
        let p = params();
        let plan = build_plan(&p, &tariff(0.3, 0.6, 1.2, 0.9)).unwrap();
        for pt in &plan.points {
            assert!(pt.power_value <= p.max_charge_kw + 1e-9);
            assert!(-pt.power_value <= p.max_discharge_kw + 1e-9);
            assert!(
                pt.soc_after >= p.soc_min - 0.05 && pt.soc_after <= p.soc_max + 0.05,
                "时段{}的SOC{}越界",
                pt.time,
                pt.soc_after
            );
        }
        assert!((plan.summary.end_soc - p.initial_soc).abs() < 0.5);
    }

    #[test]
    fn flat_prices_have_no_arbitrage() {
        let plan = build_plan(&params(), &tariff(0.6, 0.6, 0.6, 0.6)).unwrap();
        assert!(!plan.summary.has_arbitrage);
        assert!(
            plan.points
                .iter()
                .all(|p| p.power_value == 0.0 && p.soc_limit.is_none())
        );
        assert_eq!(plan.summary.net_profit, 0.0);
    }

    #[test]
    fn spread_smaller_than_conversion_loss_is_skipped() {
        // 0.5 -> 0.52：往返效率0.9时放电收入 0.52*0.9=0.468 < 充电成本 0.5
        let plan = build_plan(&params(), &tariff(0.5, 0.5, 0.52, 0.52)).unwrap();
        assert!(!plan.summary.has_arbitrage);
    }

    #[test]
    fn degradation_cost_can_kill_marginal_arbitrage() {
        let mut p = params();
        let t = tariff(0.5, 0.6, 0.9, 0.8);
        assert!(build_plan(&p, &t).unwrap().summary.has_arbitrage);
        p.degradation_cost_per_kwh = 1.0;
        assert!(!build_plan(&p, &t).unwrap().summary.has_arbitrage);
    }

    /// 谷 0-6，峰 6-10，平 10-14，尖 14-22，谷 22-24：
    /// 早高峰放完后，平段可以再充一轮供晚高峰放电
    fn double_peak_tariff(flat: f64) -> Vec<(PeriodType, f64)> {
        (0..SLOTS)
            .map(|t| match t as u32 * SLOT_MINUTES {
                m if m < 6 * 60 || m >= 22 * 60 => (PeriodType::Valley, 0.3),
                m if m < 10 * 60 => (PeriodType::Peak, 1.0),
                m if m < 14 * 60 => (PeriodType::Flat, flat),
                _ => (PeriodType::Sharp, 1.0),
            })
            .collect()
    }

    #[test]
    fn second_cycle_only_when_profitable() {
        // 平段 0.4 充、晚高峰 1.0 放：1.0*0.9 > 0.4/0.9，应出现第二轮
        let plan = build_plan(&params(), &double_peak_tariff(0.4)).unwrap();
        assert!(
            period_of(&plan, PeriodType::Flat).any(|p| p.power_value > 0.0),
            "第二轮套利应在平段充电"
        );
        // 平段 0.9 充：0.9/0.9 = 1.0 > 放电收入 0.9，不划算，平段不应充电
        let plan = build_plan(&params(), &double_peak_tariff(0.9)).unwrap();
        assert!(period_of(&plan, PeriodType::Flat).all(|p| p.power_value <= 0.0));
    }

    #[test]
    fn no_second_cycle_when_battery_is_full_before_flat() {
        // 谷充满后紧跟平段，中间没有放电窗口，平段不可能再充电
        let plan = build_plan(&params(), &tariff(0.3, 0.4, 0.5, 1.0)).unwrap();
        assert!(period_of(&plan, PeriodType::Flat).all(|p| p.power_value <= 0.0));
    }

    #[test]
    fn soc_limit_matches_direction() {
        let p = params();
        let plan = build_plan(&p, &tariff(0.3, 0.6, 1.2, 0.9)).unwrap();
        for pt in &plan.points {
            match pt.power_value {
                v if v > 0.0 => assert_eq!(pt.soc_limit, Some(p.soc_max)),
                v if v < 0.0 => assert_eq!(pt.soc_limit, Some(p.soc_min)),
                _ => assert_eq!(pt.soc_limit, None),
            }
        }
    }

    #[test]
    fn unreachable_end_soc_is_rejected() {
        let mut p = params();
        p.max_charge_kw = 1.0;
        p.end_soc = Some(90.0);
        assert!(build_plan(&p, &tariff(0.3, 0.6, 1.2, 0.9)).is_err());
    }

    #[test]
    fn invalid_soc_range_is_rejected() {
        let mut p = params();
        p.soc_min = 90.0;
        p.soc_max = 10.0;
        assert!(build_plan(&p, &tariff(0.3, 0.6, 1.2, 0.9)).is_err());
        let mut p = params();
        p.initial_soc = 95.0;
        assert!(build_plan(&p, &tariff(0.3, 0.6, 1.2, 0.9)).is_err());
    }

    #[test]
    fn off_grid_initial_soc_stays_in_bounds() {
        let mut p = params();
        p.initial_soc = 37.3;
        let plan = build_plan(&p, &tariff(0.3, 0.6, 1.2, 0.9)).unwrap();
        assert!(
            plan.points
                .iter()
                .all(|pt| pt.soc_after >= p.soc_min - 0.05 && pt.soc_after <= p.soc_max + 0.05)
        );
    }

    #[test]
    fn slot_prices_maps_periods_and_reports_gaps() {
        let periods = vec![
            ElectricityPeriod {
                start_time: "00:00".into(),
                end_time: "12:00".into(),
                period_type: PeriodType::Valley,
            },
            ElectricityPeriod {
                start_time: "12:00".into(),
                end_time: "24:00".into(),
                period_type: PeriodType::Peak,
            },
        ];
        let prices = vec![
            ElectricityPrice {
                period_type: PeriodType::Valley,
                price: 0.3,
            },
            ElectricityPrice {
                period_type: PeriodType::Peak,
                price: 1.0,
            },
        ];
        let slots = slot_prices(&periods, &prices).unwrap();
        assert_eq!(slots.len(), SLOTS);
        assert_eq!(slots[47], (PeriodType::Valley, 0.3));
        assert_eq!(slots[48], (PeriodType::Peak, 1.0));
        // 缺少峰电价
        assert!(slot_prices(&periods, &prices[..1]).is_err());
        // 时段表未覆盖全天
        assert!(slot_prices(&periods[..1], &prices).is_err());
        assert!(slot_prices(&[], &prices).is_err());
    }
}
