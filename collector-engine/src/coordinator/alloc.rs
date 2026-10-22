//! 功率分配的纯函数：不依赖 DataCenter，便于单测覆盖各种边界。

/// 一台成员可承担的范围与分配权重。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub lower: f64,
    pub upper: f64,
    pub weight: f64,
}

impl Bounds {
    /// 限值非有限数或下限大于上限（上送数据异常）时返回 `None`，该成员按不可用处理。
    pub fn new(lower: f64, upper: f64, weight: f64) -> Option<Self> {
        (lower.is_finite() && upper.is_finite() && lower <= upper).then_some(Self {
            lower,
            upper,
            weight: if weight.is_finite() && weight > 0.0 {
                weight
            } else {
                1.0
            },
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Allocation {
    /// 每台成员分到的值；不可用成员为 0
    pub values: Vec<f64>,
    /// 目标值超出总可调范围被截断的部分（目标 - 实际分配之和），0 表示完全满足
    pub unmet: f64,
}

/// 由设备上送的"最大可充""最大可放"（正数）换算出带符号的有功设定范围 `(下限, 上限)`。
/// 任一值非有限数或为负（上送异常）返回 `None`。
pub fn p_range_from_max(
    max_charge: f64,
    max_discharge: f64,
    positive_is_discharge: bool,
) -> Option<(f64, f64)> {
    let valid = |v: f64| v.is_finite() && v >= 0.0;
    if !valid(max_charge) || !valid(max_discharge) {
        return None;
    }
    Some(if positive_is_discharge {
        (-max_charge, max_discharge)
    } else {
        (-max_discharge, max_charge)
    })
}

/// 有功范围：上送的上下限，再受额定容量 `±S` 约束。
pub fn p_bounds(lower: f64, upper: f64, rated_kva: f64) -> Option<Bounds> {
    Bounds::new(lower.max(-rated_kva), upper.min(rated_kva), rated_kva)
}

/// 无功范围：上送的上下限，再受 `P² + Q² ≤ S²` 约束，`p` 为该成员的有功。
pub fn q_bounds(lower: f64, upper: f64, rated_kva: f64, p: f64) -> Option<Bounds> {
    let q_cap = (rated_kva * rated_kva - p * p).max(0.0).sqrt();
    let hi = upper.min(q_cap);
    // 上送下限若高于容量允许的上限，整体收拢到 hi，避免出现 lower > upper
    let lo = lower.max(-q_cap).min(hi);
    Bounds::new(lo, hi, rated_kva)
}

/// 按权重分配 `target`。`None` 表示该成员不可用（离线、故障、限值异常），分到 0。
///
/// 先把目标截断到总可调范围内；再按权重比例分配，某台触及上下限就钳在边界上，
/// 剩余量在其余成员间继续按比例分配，直到分完。
pub fn allocate(target: f64, members: &[Option<Bounds>]) -> Allocation {
    let mut values = vec![0.0; members.len()];
    let avail: Vec<usize> = (0..members.len())
        .filter(|&i| members[i].is_some())
        .collect();
    if avail.is_empty() || !target.is_finite() {
        return Allocation {
            values,
            unmet: if target.is_finite() { target } else { 0.0 },
        };
    }
    let b = |i: usize| members[i].unwrap();
    let total_lower: f64 = avail.iter().map(|&i| b(i).lower).sum();
    let total_upper: f64 = avail.iter().map(|&i| b(i).upper).sum();
    let goal = target.clamp(total_lower, total_upper);

    let mut free = avail.clone();
    let mut remaining = goal;
    while !free.is_empty() {
        let weights: f64 = free.iter().map(|&i| b(i).weight).sum();
        let lambda = remaining / weights;
        let ideal = |i: usize| lambda * b(i).weight;
        // 钳位后与理想值的净差：<0 说明主要是触及上限，>0 主要是触及下限
        let delta: f64 = free
            .iter()
            .map(|&i| ideal(i).clamp(b(i).lower, b(i).upper) - ideal(i))
            .sum();
        let over = |i: usize| ideal(i) > b(i).upper;
        let under = |i: usize| ideal(i) < b(i).lower;
        let fix: Vec<usize> = if delta < 0.0 {
            free.iter().copied().filter(|&i| over(i)).collect()
        } else if delta > 0.0 {
            free.iter().copied().filter(|&i| under(i)).collect()
        } else {
            free.iter()
                .copied()
                .filter(|&i| over(i) || under(i))
                .collect()
        };
        if fix.is_empty() {
            for &i in &free {
                values[i] = ideal(i);
            }
            break;
        }
        for &i in &fix {
            values[i] = ideal(i).clamp(b(i).lower, b(i).upper);
            remaining -= values[i];
        }
        free.retain(|i| !fix.contains(i));
        if delta == 0.0 {
            // 上下限钳位量相互抵消，剩余成员恰好分到理想值
            for &i in &free {
                values[i] = ideal(i);
            }
            break;
        }
    }
    let sum: f64 = values.iter().sum();
    Allocation {
        values,
        unmet: target - sum,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(lower: f64, upper: f64) -> Option<Bounds> {
        Bounds::new(lower, upper, 1.0)
    }

    fn approx(a: &[f64], e: &[f64]) {
        assert_eq!(a.len(), e.len());
        for (x, y) in a.iter().zip(e) {
            assert!((x - y).abs() < 1e-9, "{a:?} != {e:?}");
        }
    }

    #[test]
    fn equal_split() {
        let r = allocate(100.0, &[b(-500.0, 500.0), b(-500.0, 500.0)]);
        approx(&r.values, &[50.0, 50.0]);
        assert!(r.unmet.abs() < 1e-9);
    }

    #[test]
    fn weighted_split() {
        let m = [Bounds::new(-1e3, 1e3, 1.0), Bounds::new(-1e3, 1e3, 3.0)];
        approx(&allocate(100.0, &m).values, &[25.0, 75.0]);
    }

    #[test]
    fn saturated_member_pushes_rest_to_other() {
        // A 上限只有 30，剩余 70 全给 B
        let r = allocate(100.0, &[b(-500.0, 30.0), b(-500.0, 500.0)]);
        approx(&r.values, &[30.0, 70.0]);
        assert!(r.unmet.abs() < 1e-9);
    }

    #[test]
    fn target_beyond_total_is_truncated() {
        let r = allocate(1000.0, &[b(-100.0, 200.0), b(-100.0, 300.0)]);
        approx(&r.values, &[200.0, 300.0]);
        assert!((r.unmet - 500.0).abs() < 1e-9);
    }

    #[test]
    fn negative_target_charge() {
        let r = allocate(-100.0, &[b(-30.0, 500.0), b(-500.0, 500.0)]);
        approx(&r.values, &[-30.0, -70.0]);
        let r = allocate(-1000.0, &[b(-100.0, 200.0), b(-100.0, 300.0)]);
        approx(&r.values, &[-100.0, -100.0]);
        assert!((r.unmet + 800.0).abs() < 1e-9);
    }

    #[test]
    fn lower_bound_above_zero() {
        // 两台都有正的最小出力，目标 0 也要落在各自下限
        let r = allocate(0.0, &[b(10.0, 100.0), b(20.0, 100.0)]);
        approx(&r.values, &[10.0, 20.0]);
        assert!((r.unmet + 30.0).abs() < 1e-9);
    }

    #[test]
    fn offline_member_gets_zero() {
        let r = allocate(100.0, &[None, b(-500.0, 500.0)]);
        approx(&r.values, &[0.0, 100.0]);
        let r = allocate(800.0, &[None, b(-500.0, 500.0)]);
        approx(&r.values, &[0.0, 500.0]);
        assert!((r.unmet - 300.0).abs() < 1e-9);
    }

    #[test]
    fn all_offline_or_bad_target() {
        let r = allocate(100.0, &[None, None]);
        approx(&r.values, &[0.0, 0.0]);
        assert_eq!(r.unmet, 100.0);
        let r = allocate(f64::NAN, &[b(0.0, 1.0)]);
        approx(&r.values, &[0.0]);
        assert_eq!(r.unmet, 0.0);
    }

    #[test]
    fn invalid_bounds_rejected() {
        assert!(Bounds::new(10.0, 5.0, 1.0).is_none());
        assert!(Bounds::new(f64::NAN, 5.0, 1.0).is_none());
        // 权重非法回退为 1
        assert_eq!(Bounds::new(0.0, 1.0, 0.0).unwrap().weight, 1.0);
    }

    #[test]
    fn sum_always_matches_when_feasible() {
        let m = [b(-50.0, 120.0), b(-200.0, 80.0), b(-10.0, 400.0)];
        for t in [-260.0, -100.0, 0.0, 33.3, 250.0, 600.0] {
            let r = allocate(t, &m);
            let sum: f64 = r.values.iter().sum();
            assert!((sum - t).abs() < 1e-6, "t={t} sum={sum} {:?}", r.values);
            for (v, bd) in r.values.iter().zip(&m) {
                let bd = bd.unwrap();
                assert!(*v >= bd.lower - 1e-9 && *v <= bd.upper + 1e-9);
            }
        }
    }

    #[test]
    fn q_limited_by_capacity_ellipse() {
        // S=500、P=400 时 Q 最多 ±300
        let q = q_bounds(-1000.0, 1000.0, 500.0, 400.0).unwrap();
        assert!((q.upper - 300.0).abs() < 1e-9 && (q.lower + 300.0).abs() < 1e-9);
        // 上送限值更紧时取上送限值
        let q = q_bounds(-100.0, 50.0, 500.0, 400.0).unwrap();
        assert_eq!((q.lower, q.upper), (-100.0, 50.0));
        // P 已满容量，无功只能为 0
        let q = q_bounds(-100.0, 100.0, 500.0, 500.0).unwrap();
        assert_eq!((q.lower, q.upper), (0.0, 0.0));
        // 上送下限高于容量允许的上限，收拢而不是 lower>upper
        let q = q_bounds(350.0, 400.0, 500.0, 400.0).unwrap();
        assert_eq!((q.lower, q.upper), (300.0, 300.0));
        // 下限仍在容量范围内时保持不变
        let q = q_bounds(200.0, 400.0, 500.0, 400.0).unwrap();
        assert_eq!((q.lower, q.upper), (200.0, 300.0));
    }

    #[test]
    fn range_from_max_follows_sign_convention() {
        // 正放负充：上限=可放，下限=-可充
        assert_eq!(p_range_from_max(300.0, 500.0, true), Some((-300.0, 500.0)));
        // 正充负放：上限=可充，下限=-可放
        assert_eq!(p_range_from_max(300.0, 500.0, false), Some((-500.0, 300.0)));
        // 满了不能再充：可充为 0
        assert_eq!(p_range_from_max(0.0, 500.0, true), Some((0.0, 500.0)));
        // 异常上送
        assert_eq!(p_range_from_max(-1.0, 500.0, true), None);
        assert_eq!(p_range_from_max(300.0, f64::NAN, true), None);
        assert_eq!(p_range_from_max(f64::INFINITY, 1.0, false), None);
    }

    #[test]
    fn p_limited_by_rated() {
        let p = p_bounds(-900.0, 900.0, 500.0).unwrap();
        assert_eq!((p.lower, p.upper), (-500.0, 500.0));
    }
}
