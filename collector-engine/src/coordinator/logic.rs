//! 协调控制器的状态机：一次 `step` 输入各成员的当前视图，输出要下发的设定值和要上送的聚合量。
//! 不依赖 DataCenter，单测直接驱动。

use std::time::{Duration, Instant};

use super::alloc::{Bounds, allocate, p_bounds, q_bounds};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    P,
    Q,
}

/// 一台成员此刻的状态；`online=false` 时其余字段无意义。
#[derive(Debug, Clone, Copy, Default)]
pub struct MemberView {
    pub online: bool,
    pub p_actual: f64,
    pub q_actual: f64,
    pub p_lower: f64,
    pub p_upper: f64,
    pub q_lower: f64,
    pub q_upper: f64,
}

pub struct Config {
    pub rated_kva: Vec<f64>,
    pub p_deadband: f64,
    pub q_deadband: f64,
    /// `None` 表示不启用上级失联清零
    pub upstream_timeout: Option<Duration>,
}

/// 来自北向或本地的设置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Setting {
    PTarget(f64),
    QTarget(f64),
    AgcEnable(bool),
    AvcEnable(bool),
    AgcRemote(bool),
    AvcRemote(bool),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Telemetry {
    pub total_p: f64,
    pub total_q: f64,
    pub p_upper: f64,
    pub p_lower: f64,
    pub q_upper: f64,
    pub q_lower: f64,
    pub p_unmet: f64,
    pub q_unmet: f64,
    pub all_online: bool,
}

#[derive(Debug)]
pub struct Output {
    /// (成员下标, 有功/无功, 设定值)
    pub sends: Vec<(usize, Kind, f64)>,
    pub telemetry: Telemetry,
}

#[derive(Debug)]
pub struct State {
    pub agc_enable: bool,
    pub avc_enable: bool,
    /// true=远方，false=就地
    pub agc_remote: bool,
    pub avc_remote: bool,
    pub p_target: f64,
    pub q_target: f64,
    prev_agc: bool,
    prev_avc: bool,
    last_p: Vec<Option<f64>>,
    last_q: Vec<Option<f64>>,
    /// 退出后还没成功清零的成员
    pending_zero_p: Vec<bool>,
    pending_zero_q: Vec<bool>,
    /// 北向连接全部断开的起点
    lost_since: Option<Instant>,
    timed_out: bool,
}

impl State {
    /// 启动时：AGC/AVC 退出、远方、设定值为 0（重启不恢复上次设定）。
    pub fn new(members: usize) -> Self {
        Self {
            agc_enable: false,
            avc_enable: false,
            agc_remote: true,
            avc_remote: true,
            p_target: 0.0,
            q_target: 0.0,
            prev_agc: false,
            prev_avc: false,
            last_p: vec![None; members],
            last_q: vec![None; members],
            pending_zero_p: vec![false; members],
            pending_zero_q: vec![false; members],
            lost_since: None,
            timed_out: false,
        }
    }

    /// 北向是否可以修改总设定值：必须处于远方且已投入。
    pub fn remote_may_set(&self, kind: Kind) -> bool {
        match kind {
            Kind::P => self.agc_remote && self.agc_enable,
            Kind::Q => self.avc_remote && self.avc_enable,
        }
    }

    /// 应用一条设置。退出状态下的设定值没有意义，直接拒绝。
    pub fn apply(&mut self, s: Setting) -> Result<(), &'static str> {
        match s {
            Setting::PTarget(v) | Setting::QTarget(v) if !v.is_finite() => Err("设定值非法"),
            Setting::PTarget(_) if !self.agc_enable => Err("AGC未投入"),
            Setting::QTarget(_) if !self.avc_enable => Err("AVC未投入"),
            Setting::PTarget(v) => {
                self.p_target = v;
                Ok(())
            }
            Setting::QTarget(v) => {
                self.q_target = v;
                Ok(())
            }
            Setting::AgcEnable(v) => {
                self.agc_enable = v;
                Ok(())
            }
            Setting::AvcEnable(v) => {
                self.avc_enable = v;
                Ok(())
            }
            Setting::AgcRemote(v) => {
                self.agc_remote = v;
                Ok(())
            }
            Setting::AvcRemote(v) => {
                self.avc_remote = v;
                Ok(())
            }
        }
    }

    /// 远方 + 投入状态下，北向连接全部断开超过 `timeout` 则总设定值清零。
    fn check_upstream(&mut self, cfg: &Config, masters: Option<u32>, now: Instant) {
        let (Some(timeout), Some(0)) = (cfg.upstream_timeout, masters) else {
            // 没配超时、北向没运行、或有连接：复位计时
            self.lost_since = None;
            self.timed_out = false;
            return;
        };
        let since = *self.lost_since.get_or_insert(now);
        if !self.timed_out && now.duration_since(since) >= timeout {
            self.timed_out = true;
            if self.remote_may_set(Kind::P) {
                self.p_target = 0.0;
            }
            if self.remote_may_set(Kind::Q) {
                self.q_target = 0.0;
            }
            tracing::warn!("[协调控制器] 上级失联超过 {:?}, 总设定值已清零", timeout);
        }
    }

    pub fn step(
        &mut self,
        cfg: &Config,
        views: &[MemberView],
        masters: Option<u32>,
        now: Instant,
    ) -> Output {
        let n = views.len();
        self.check_upstream(cfg, masters, now);

        // 退出：设定值和各成员都回到 0；投入：从 0 开始
        if self.prev_agc != self.agc_enable {
            self.p_target = 0.0;
            self.pending_zero_p = vec![!self.agc_enable; n];
        }
        if self.prev_avc != self.avc_enable {
            self.q_target = 0.0;
            self.pending_zero_q = vec![!self.avc_enable; n];
        }
        self.prev_agc = self.agc_enable;
        self.prev_avc = self.avc_enable;

        let rated = |i: usize| cfg.rated_kva[i];
        let online = |i: usize| views[i].online;

        // 有功
        let p_b: Vec<Option<Bounds>> = (0..n)
            .map(|i| {
                online(i)
                    .then(|| p_bounds(views[i].p_lower, views[i].p_upper, rated(i)))
                    .flatten()
            })
            .collect();
        let p_alloc = self.agc_enable.then(|| allocate(self.p_target, &p_b));
        // 无功约束要用到有功：已分配则用分配值，否则用实测值
        let p_ref = |i: usize| match &p_alloc {
            Some(a) => a.values[i],
            None => views[i].p_actual,
        };
        let q_b: Vec<Option<Bounds>> = (0..n)
            .map(|i| {
                online(i)
                    .then(|| q_bounds(views[i].q_lower, views[i].q_upper, rated(i), p_ref(i)))
                    .flatten()
            })
            .collect();
        let q_alloc = self.avc_enable.then(|| allocate(self.q_target, &q_b));

        let mut sends = Vec::new();
        let mut plan = |kind: Kind, enabled: bool, alloc: &Option<super::alloc::Allocation>| {
            let (last, pending, deadband, avail) = match kind {
                Kind::P => (
                    &mut self.last_p,
                    &mut self.pending_zero_p,
                    cfg.p_deadband,
                    &p_b,
                ),
                Kind::Q => (
                    &mut self.last_q,
                    &mut self.pending_zero_q,
                    cfg.q_deadband,
                    &q_b,
                ),
            };
            for i in 0..n {
                if !online(i) {
                    // 离线后恢复时要重新下发
                    last[i] = None;
                    continue;
                }
                if enabled {
                    let Some(a) = alloc else { continue };
                    if avail[i].is_none() {
                        continue;
                    }
                    let v = a.values[i];
                    let changed = match last[i] {
                        None => true,
                        Some(old) => (v - old).abs() >= deadband || (v == 0.0 && old != 0.0),
                    };
                    if changed {
                        last[i] = Some(v);
                        sends.push((i, kind, v));
                    }
                } else if pending[i] {
                    pending[i] = false;
                    last[i] = Some(0.0);
                    sends.push((i, kind, 0.0));
                }
            }
        };
        plan(Kind::P, self.agc_enable, &p_alloc);
        plan(Kind::Q, self.avc_enable, &q_alloc);

        let sum = |f: &dyn Fn(usize) -> f64| (0..n).filter(|&i| online(i)).map(f).sum::<f64>();
        let range = |b: &[Option<Bounds>]| {
            (
                b.iter().flatten().map(|x| x.upper).sum::<f64>(),
                b.iter().flatten().map(|x| x.lower).sum::<f64>(),
            )
        };
        let (p_upper, p_lower) = range(&p_b);
        let (q_upper, q_lower) = range(&q_b);
        let telemetry = Telemetry {
            total_p: sum(&|i| views[i].p_actual),
            total_q: sum(&|i| views[i].q_actual),
            p_upper,
            p_lower,
            q_upper,
            q_lower,
            p_unmet: p_alloc.as_ref().map_or(0.0, |a| a.unmet),
            q_unmet: q_alloc.as_ref().map_or(0.0, |a| a.unmet),
            all_online: views.iter().all(|v| v.online),
        };
        Output { sends, telemetry }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: f64 = 500.0;

    fn cfg(timeout: Option<Duration>) -> Config {
        Config {
            rated_kva: vec![S, S],
            p_deadband: 1.0,
            q_deadband: 1.0,
            upstream_timeout: timeout,
        }
    }

    fn view() -> MemberView {
        MemberView {
            online: true,
            p_actual: 0.0,
            q_actual: 0.0,
            p_lower: -S,
            p_upper: S,
            q_lower: -S,
            q_upper: S,
        }
    }

    fn sent(o: &Output, kind: Kind) -> Vec<(usize, f64)> {
        o.sends
            .iter()
            .filter(|s| s.1 == kind)
            .map(|s| (s.0, s.2))
            .collect()
    }

    fn enabled() -> State {
        let mut s = State::new(2);
        s.apply(Setting::AgcEnable(true)).unwrap();
        s.apply(Setting::AvcEnable(true)).unwrap();
        s
    }

    #[test]
    fn starts_exited_remote_and_zero() {
        let mut s = State::new(2);
        assert!(s.agc_remote && s.avc_remote && !s.agc_enable && !s.avc_enable);
        let o = s.step(&cfg(None), &[view(), view()], None, Instant::now());
        assert!(o.sends.is_empty());
        // 退出状态不接受设定值
        assert!(s.apply(Setting::PTarget(10.0)).is_err());
        assert!(s.apply(Setting::QTarget(10.0)).is_err());
        assert!(!s.remote_may_set(Kind::P));
    }

    #[test]
    fn allocates_p_and_q_equally() {
        let (c, now) = (cfg(None), Instant::now());
        let mut s = enabled();
        s.step(&c, &[view(), view()], None, now);
        s.apply(Setting::PTarget(200.0)).unwrap();
        s.apply(Setting::QTarget(-60.0)).unwrap();
        let o = s.step(&c, &[view(), view()], None, now);
        assert_eq!(sent(&o, Kind::P), vec![(0, 100.0), (1, 100.0)]);
        assert_eq!(sent(&o, Kind::Q), vec![(0, -30.0), (1, -30.0)]);
        assert_eq!(o.telemetry.p_unmet, 0.0);
        assert_eq!(
            (o.telemetry.p_upper, o.telemetry.p_lower),
            (1000.0, -1000.0)
        );
    }

    #[test]
    fn deadband_and_keep_setpoint() {
        let (c, now) = (cfg(None), Instant::now());
        let mut s = enabled();
        s.step(&c, &[view(), view()], None, now);
        s.apply(Setting::PTarget(200.0)).unwrap();
        s.step(&c, &[view(), view()], None, now);
        // 设定值保持，未变化不重复下发
        assert!(sent(&s.step(&c, &[view(), view()], None, now), Kind::P).is_empty());
        // 变化小于死区(每台 0.4)不下发
        s.apply(Setting::PTarget(200.8)).unwrap();
        assert!(sent(&s.step(&c, &[view(), view()], None, now), Kind::P).is_empty());
        s.apply(Setting::PTarget(210.0)).unwrap();
        assert_eq!(
            sent(&s.step(&c, &[view(), view()], None, now), Kind::P),
            vec![(0, 105.0), (1, 105.0)]
        );
        // 降到 0 即使在死区内也要下发
        s.apply(Setting::PTarget(0.0)).unwrap();
        assert_eq!(
            sent(&s.step(&c, &[view(), view()], None, now), Kind::P),
            vec![(0, 0.0), (1, 0.0)]
        );
    }

    #[test]
    fn member_offline_truncates_then_redistributes_on_recovery() {
        let (c, now) = (cfg(None), Instant::now());
        let mut s = enabled();
        s.step(&c, &[view(), view()], None, now);
        s.apply(Setting::PTarget(800.0)).unwrap();
        let o = s.step(&c, &[view(), view()], None, now);
        assert_eq!(sent(&o, Kind::P), vec![(0, 400.0), (1, 400.0)]);
        // A 离线：B 截断到上限 500，未满足 300；不向离线的 A 下发
        let off = MemberView::default();
        let o = s.step(&c, &[off, view()], None, now);
        assert_eq!(sent(&o, Kind::P), vec![(1, 500.0)]);
        assert_eq!(o.telemetry.p_unmet, 300.0);
        assert!(!o.telemetry.all_online);
        // A 恢复：设定值保持，自动重新分配并给 A 重新下发
        let o = s.step(&c, &[view(), view()], None, now);
        assert_eq!(sent(&o, Kind::P), vec![(0, 400.0), (1, 400.0)]);
    }

    #[test]
    fn q_respects_capacity_after_p() {
        let (c, now) = (cfg(None), Instant::now());
        let mut s = enabled();
        s.step(&c, &[view(), view()], None, now);
        s.apply(Setting::PTarget(800.0)).unwrap(); // 每台 400
        s.apply(Setting::QTarget(1000.0)).unwrap(); // 每台最多 √(500²-400²)=300
        let o = s.step(&c, &[view(), view()], None, now);
        assert_eq!(sent(&o, Kind::Q), vec![(0, 300.0), (1, 300.0)]);
        assert_eq!(o.telemetry.q_unmet, 400.0);
    }

    #[test]
    fn exit_returns_to_zero_and_retries_offline_member() {
        let (c, now) = (cfg(None), Instant::now());
        let mut s = enabled();
        s.step(&c, &[view(), view()], None, now);
        s.apply(Setting::PTarget(200.0)).unwrap();
        s.apply(Setting::QTarget(100.0)).unwrap();
        s.step(&c, &[view(), view()], None, now);
        // 退出 AGC 时 A 离线
        s.apply(Setting::AgcEnable(false)).unwrap();
        let off = MemberView::default();
        let o = s.step(&c, &[off, view()], None, now);
        assert_eq!(s.p_target, 0.0);
        assert_eq!(sent(&o, Kind::P), vec![(1, 0.0)]);
        // AVC 仍投入：退出 AGC 不影响无功，B 接手离线的 A 那份（50 -> 100）
        assert_eq!(sent(&o, Kind::Q), vec![(1, 100.0)]);
        // A 恢复后补发一次 0，之后不再干预
        let o = s.step(&c, &[view(), view()], None, now);
        assert_eq!(sent(&o, Kind::P), vec![(0, 0.0)]);
        assert!(sent(&s.step(&c, &[view(), view()], None, now), Kind::P).is_empty());
        // 重新投入从 0 开始
        s.apply(Setting::AgcEnable(true)).unwrap();
        s.step(&c, &[view(), view()], None, now);
        assert_eq!(s.p_target, 0.0);
    }

    #[test]
    fn upstream_loss_clears_only_in_remote_and_after_timeout() {
        let c = cfg(Some(Duration::from_secs(60)));
        let t0 = Instant::now();
        let mut s = enabled();
        s.step(&c, &[view(), view()], Some(1), t0);
        s.apply(Setting::PTarget(200.0)).unwrap();
        s.apply(Setting::QTarget(100.0)).unwrap();
        // 刚断开：不清零
        s.step(&c, &[view(), view()], Some(0), t0);
        s.step(&c, &[view(), view()], Some(0), t0 + Duration::from_secs(59));
        assert_eq!(s.p_target, 200.0);
        // 超时：清零并下发
        let o = s.step(&c, &[view(), view()], Some(0), t0 + Duration::from_secs(61));
        assert_eq!((s.p_target, s.q_target), (0.0, 0.0));
        assert_eq!(sent(&o, Kind::P), vec![(0, 0.0), (1, 0.0)]);
        // 重连后上级重新下发，不会再被误清
        s.step(&c, &[view(), view()], Some(1), t0 + Duration::from_secs(62));
        s.apply(Setting::PTarget(50.0)).unwrap();
        s.step(
            &c,
            &[view(), view()],
            Some(1),
            t0 + Duration::from_secs(200),
        );
        assert_eq!(s.p_target, 50.0);
        // 北向没运行 / 就地：不清零
        let mut s = enabled();
        s.step(&c, &[view(), view()], None, t0);
        s.apply(Setting::PTarget(70.0)).unwrap();
        s.step(&c, &[view(), view()], None, t0 + Duration::from_secs(500));
        assert_eq!(s.p_target, 70.0);
        s.apply(Setting::AgcRemote(false)).unwrap();
        s.step(&c, &[view(), view()], Some(0), t0);
        s.step(
            &c,
            &[view(), view()],
            Some(0),
            t0 + Duration::from_secs(500),
        );
        assert_eq!(s.p_target, 70.0, "就地设定值不受上级失联影响");
    }

    #[test]
    fn remote_gate_requires_remote_and_enabled() {
        let mut s = State::new(2);
        assert!(!s.remote_may_set(Kind::P));
        s.apply(Setting::AgcEnable(true)).unwrap();
        assert!(s.remote_may_set(Kind::P));
        s.apply(Setting::AgcRemote(false)).unwrap();
        assert!(!s.remote_may_set(Kind::P));
        // AVC 与 AGC 互不影响
        assert!(!s.remote_may_set(Kind::Q));
        s.apply(Setting::AvcEnable(true)).unwrap();
        assert!(s.remote_may_set(Kind::Q));
    }

    #[test]
    fn telemetry_sums_only_online_members() {
        let (c, now) = (cfg(None), Instant::now());
        let mut s = State::new(2);
        let a = MemberView {
            p_actual: 120.0,
            q_actual: 10.0,
            ..view()
        };
        let b = MemberView {
            p_actual: 80.0,
            q_actual: -4.0,
            ..view()
        };
        let o = s.step(&c, &[a, b], None, now);
        assert_eq!((o.telemetry.total_p, o.telemetry.total_q), (200.0, 6.0));
        let o = s.step(&c, &[MemberView::default(), b], None, now);
        assert_eq!((o.telemetry.total_p, o.telemetry.total_q), (80.0, -4.0));
    }
}
