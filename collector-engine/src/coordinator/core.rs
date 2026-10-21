//! 协调控制器虚拟设备：订阅下级、聚合上送，收到总设定值后分配下发。

use std::{sync::Arc, time::Duration};

use collector_core::{
    center::{DataCenterError, data_center},
    config::{
        iec104_conf::COMM_STATUS_ID,
        program_conf::{CoordinatorMember, CoordinatorProgram},
    },
    core::point::{DataPoint, DownDataPoint, PointId, PointRef, Val},
    dev::{DeviceError, Executable, Identifiable, Lifecycle, LifecycleState, state::SharedState},
    dispatch::{NorthboundGate, register_northbound_gate},
};
use parking_lot::Mutex;
use tokio::{
    sync::{Notify, mpsc, watch},
    task::JoinHandle,
    time,
};

use super::logic::{Config, Kind, MemberView, Output, Setting, State};

/// 协调控制器在 DataCenter 里的点位：北向点表的 `point_id` 填这里的编号。
/// 1~6 可读可写（写入后回读即回显），10 以上只读。
struct Pt {
    id: PointId,
    key: &'static str,
    name: &'static str,
}

macro_rules! pt {
    ($c:ident, $id:expr, $key:expr, $name:expr) => {
        const $c: Pt = Pt {
            id: $id,
            key: $key,
            name: $name,
        };
    };
}

pt!(P_SET, 1, "agc_p_set", "AGC总有功设定");
pt!(Q_SET, 2, "avc_q_set", "AVC总无功设定");
pt!(AGC_ENABLE, 3, "agc_enable", "AGC投入(1投入0退出)");
pt!(AVC_ENABLE, 4, "avc_enable", "AVC投入(1投入0退出)");
pt!(AGC_REMOTE, 5, "agc_remote", "AGC远方(1远方0就地)");
pt!(AVC_REMOTE, 6, "avc_remote", "AVC远方(1远方0就地)");
pt!(TOTAL_P, 10, "total_p", "总有功功率");
pt!(TOTAL_Q, 11, "total_q", "总无功功率");
pt!(P_UPPER, 12, "p_upper", "有功可调上限");
pt!(P_LOWER, 13, "p_lower", "有功可调下限");
pt!(Q_UPPER, 14, "q_upper", "无功可调上限");
pt!(Q_LOWER, 15, "q_lower", "无功可调下限");
pt!(AGC_INC_BLOCK, 16, "agc_inc_block", "AGC增闭锁");
pt!(AGC_DEC_BLOCK, 17, "agc_dec_block", "AGC减闭锁");
pt!(AVC_INC_BLOCK, 18, "avc_inc_block", "AVC增闭锁");
pt!(AVC_DEC_BLOCK, 19, "avc_dec_block", "AVC减闭锁");
pt!(P_UNMET, 20, "agc_unmet", "AGC未满足有功");
pt!(Q_UNMET, 21, "avc_unmet", "AVC未满足无功");

impl Pt {
    fn matches(&self, p: &PointRef) -> bool {
        match p {
            PointRef::Id(id) => *id == self.id,
            PointRef::Key(k) => k == self.key,
            PointRef::Name(_) => false,
        }
    }

    fn point(&self, value: Val) -> DataPoint {
        DataPoint {
            id: self.id,
            key: self.key,
            name: self.name,
            value,
            translator: None,
            bits: None,
            words: None,
            unit: None,
            level: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CoordinatorError {
    #[error("协调控制器没有配置成员")]
    NoMembers,
    #[error("成员 {0} 的 rated_kva 必须大于 0")]
    BadRated(String),
    #[error("tick_ms 必须大于 0")]
    BadTick,
}

struct Inner {
    id: String,
    members: Vec<CoordinatorMember>,
    cfg: Config,
    tick: Duration,
    state: Mutex<State>,
    notify: Notify,
}

pub struct Coordinator {
    inner: Arc<Inner>,
    state: SharedState,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
    handles: Mutex<Vec<JoinHandle<()>>>,
}

/// 北向门禁：总设定值只在"远方 + 投入"时接受，投入/退出、远方/就地开关始终可写。
struct Gate(Arc<Inner>);

impl NorthboundGate for Gate {
    fn allow(&self, dev_id: &str, point: &DownDataPoint) -> bool {
        if dev_id != self.0.id {
            return true;
        }
        let kind = if P_SET.matches(&point.point) {
            Kind::P
        } else if Q_SET.matches(&point.point) {
            Kind::Q
        } else {
            return true;
        };
        self.0.state.lock().remote_may_set(kind)
    }
}

impl Coordinator {
    pub fn new(program: &CoordinatorProgram) -> Result<Self, CoordinatorError> {
        if program.members.is_empty() {
            return Err(CoordinatorError::NoMembers);
        }
        if program.tick_ms == 0 {
            return Err(CoordinatorError::BadTick);
        }
        if let Some(m) = program
            .members
            .iter()
            .find(|m| !(m.rated_kva.is_finite() && m.rated_kva > 0.0))
        {
            return Err(CoordinatorError::BadRated(m.dev.clone()));
        }
        let inner = Arc::new(Inner {
            id: program.id.clone(),
            members: program.members.clone(),
            cfg: Config {
                rated_kva: program.members.iter().map(|m| m.rated_kva).collect(),
                p_deadband: program.p_deadband,
                q_deadband: program.q_deadband,
                upstream_timeout: (program.upstream_timeout_ms > 0)
                    .then(|| Duration::from_millis(program.upstream_timeout_ms)),
            },
            tick: Duration::from_millis(program.tick_ms),
            state: Mutex::new(State::new(program.members.len())),
            notify: Notify::new(),
        });
        register_northbound_gate(Arc::new(Gate(inner.clone())));
        let (stop_tx, stop_rx) = watch::channel(false);
        Ok(Self {
            inner,
            state: SharedState::new(LifecycleState::New),
            stop_tx,
            stop_rx,
            handles: Mutex::new(Vec::new()),
        })
    }

    fn id(&self) -> &str {
        &self.inner.id
    }
}

impl Inner {
    fn view(&self, m: &CoordinatorMember) -> MemberView {
        let center = data_center();
        let get = |id: PointId| center.read(&m.dev, id).and_then(|p| p.value.as_f64().ok());
        let comm_fault = center
            .read(&m.dev, COMM_STATUS_ID)
            .and_then(|p| p.value.as_bool().ok())
            .unwrap_or(false);
        // 限值点还没上送，或通讯故障，都视为离线
        match (
            get(m.p_upper),
            get(m.p_lower),
            get(m.q_upper),
            get(m.q_lower),
        ) {
            (Some(p_upper), Some(p_lower), Some(q_upper), Some(q_lower)) if !comm_fault => {
                MemberView {
                    online: true,
                    p_actual: get(m.p_actual).unwrap_or(0.0),
                    q_actual: get(m.q_actual).unwrap_or(0.0),
                    p_lower,
                    p_upper,
                    q_lower,
                    q_upper,
                }
            }
            _ => MemberView::default(),
        }
    }

    async fn tick(&self) {
        let views: Vec<MemberView> = self.members.iter().map(|m| self.view(m)).collect();
        let masters = collector_core::dock::iec104::connected_masters();
        let (out, snap) = {
            let mut st = self.state.lock();
            let out = st.step(&self.cfg, &views, masters, std::time::Instant::now());
            let snap = Snapshot {
                agc_enable: st.agc_enable,
                avc_enable: st.avc_enable,
                agc_remote: st.agc_remote,
                avc_remote: st.avc_remote,
                p_target: st.p_target,
                q_target: st.q_target,
            };
            (out, snap)
        };
        self.send(&out).await;
        self.publish(&out, &snap);
    }

    /// 同一成员的有功、无功合并成一次下发。
    async fn send(&self, out: &Output) {
        for (i, m) in self.members.iter().enumerate() {
            let points: Vec<DownDataPoint> = out
                .sends
                .iter()
                .filter(|s| s.0 == i)
                .map(|&(_, kind, v)| {
                    let id = if kind == Kind::P { m.p_set } else { m.q_set };
                    DownDataPoint::by_id(id, Val::F64(v))
                })
                .collect();
            if points.is_empty() {
                continue;
            }
            tracing::info!(
                "[协调控制器] ↓ {} {:?}",
                m.dev,
                out.sends
                    .iter()
                    .filter(|s| s.0 == i)
                    .map(|s| (s.1, s.2))
                    .collect::<Vec<_>>()
            );
            let res = time::timeout(
                Duration::from_secs(2),
                data_center().dispatch(&m.dev, points),
            )
            .await;
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::error!("[协调控制器] 下发 {} 失败: {}", m.dev, e),
                Err(_) => tracing::error!("[协调控制器] 下发 {} 超时", m.dev),
            }
        }
    }

    fn publish(&self, out: &Output, s: &Snapshot) {
        let t = &out.telemetry;
        let eps = 1e-6;
        let f = |v: f64| Val::F64(v);
        let b = |v: bool| Val::U8(u8::from(v));
        let points = vec![
            P_SET.point(f(s.p_target)),
            Q_SET.point(f(s.q_target)),
            AGC_ENABLE.point(b(s.agc_enable)),
            AVC_ENABLE.point(b(s.avc_enable)),
            AGC_REMOTE.point(b(s.agc_remote)),
            AVC_REMOTE.point(b(s.avc_remote)),
            TOTAL_P.point(f(t.total_p)),
            TOTAL_Q.point(f(t.total_q)),
            P_UPPER.point(f(t.p_upper)),
            P_LOWER.point(f(t.p_lower)),
            Q_UPPER.point(f(t.q_upper)),
            Q_LOWER.point(f(t.q_lower)),
            // 闭锁：目标已到可调范围边界，无法再往该方向调节
            AGC_INC_BLOCK.point(b(s.agc_enable && s.p_target >= t.p_upper - eps)),
            AGC_DEC_BLOCK.point(b(s.agc_enable && s.p_target <= t.p_lower + eps)),
            AVC_INC_BLOCK.point(b(s.avc_enable && s.q_target >= t.q_upper - eps)),
            AVC_DEC_BLOCK.point(b(s.avc_enable && s.q_target <= t.q_lower + eps)),
            P_UNMET.point(f(t.p_unmet)),
            Q_UNMET.point(f(t.q_unmet)),
            DataPoint {
                id: COMM_STATUS_ID,
                key: "communicationStatus",
                name: "通讯状态",
                // 任一成员离线即视为故障
                value: Val::U8(u8::from(!t.all_online)),
                translator: None,
                bits: None,
                words: None,
                unit: None,
                level: None,
            },
        ];
        data_center().ingest(&self.id, points);
    }

    fn parse(p: &DownDataPoint) -> Option<Setting> {
        let num = || p.value.as_f64().ok();
        let flag = || p.value.as_bool().ok();
        if P_SET.matches(&p.point) {
            num().map(Setting::PTarget)
        } else if Q_SET.matches(&p.point) {
            num().map(Setting::QTarget)
        } else if AGC_ENABLE.matches(&p.point) {
            flag().map(Setting::AgcEnable)
        } else if AVC_ENABLE.matches(&p.point) {
            flag().map(Setting::AvcEnable)
        } else if AGC_REMOTE.matches(&p.point) {
            flag().map(Setting::AgcRemote)
        } else if AVC_REMOTE.matches(&p.point) {
            flag().map(Setting::AvcRemote)
        } else {
            None
        }
    }

    fn down(&self, points: &[DownDataPoint]) {
        let mut st = self.state.lock();
        for p in points {
            let Some(setting) = Self::parse(p) else {
                tracing::warn!("[协调控制器] 忽略不支持的下发点 {:?}", p.point);
                continue;
            };
            match st.apply(setting) {
                Ok(()) => tracing::info!("[协调控制器] 设置 {:?}", setting),
                Err(why) => tracing::warn!("[协调控制器] 拒绝 {:?}: {}", setting, why),
            }
        }
        drop(st);
        // 立刻重算并回显，不等下一个周期
        self.notify.notify_one();
    }
}

struct Snapshot {
    agc_enable: bool,
    avc_enable: bool,
    agc_remote: bool,
    avc_remote: bool,
    p_target: f64,
    q_target: f64,
}

async fn wait_for_stop(stop_rx: &mut watch::Receiver<bool>) {
    loop {
        if *stop_rx.borrow() || stop_rx.changed().await.is_err() {
            return;
        }
    }
}

async fn run_tick(inner: Arc<Inner>, mut stop_rx: watch::Receiver<bool>) {
    let mut ticker = time::interval(inner.tick);
    ticker.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            _ = wait_for_stop(&mut stop_rx) => break,
            _ = ticker.tick() => {}
            _ = inner.notify.notified() => {}
        }
        inner.tick().await;
    }
}

async fn run_downlink(
    inner: Arc<Inner>,
    mut rx: mpsc::Receiver<Vec<DownDataPoint>>,
    mut stop_rx: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            _ = wait_for_stop(&mut stop_rx) => break,
            msg = rx.recv() => match msg {
                Some(points) => inner.down(&points),
                None => break,
            },
        }
    }
}

#[async_trait::async_trait]
impl Lifecycle for Coordinator {
    fn init(&self) -> Result<(), DeviceError> {
        if self
            .state
            .cas(LifecycleState::New, LifecycleState::Initializing)
        {
            self.state.store(self.id(), LifecycleState::Ready);
        }
        Ok(())
    }

    async fn start(&mut self) -> Result<(), DeviceError> {
        let ok = self
            .state
            .cas(LifecycleState::Ready, LifecycleState::Starting)
            || self
                .state
                .cas(LifecycleState::Stopped, LifecycleState::Starting);
        if !ok {
            return Ok(());
        }
        let (tx, rx) = mpsc::channel::<Vec<DownDataPoint>>(16);
        match data_center().attach_downlink(self.id(), tx.clone()) {
            Ok(()) => {}
            Err(DataCenterError::DevHasRegister(_)) => {
                data_center().detach_downlink(self.id());
                if let Err(err) = data_center().attach_downlink(self.id(), tx) {
                    tracing::warn!("[{}] 重新注册数据中心失败: {}", self.id(), err);
                    self.state.store(self.id(), LifecycleState::Failed);
                    return Ok(());
                }
            }
            Err(err) => {
                tracing::warn!("[{}] 注册数据中心失败: {}", self.id(), err);
                self.state.store(self.id(), LifecycleState::Failed);
                return Ok(());
            }
        }
        let _ = self.stop_tx.send(false);
        // 先上送一次初始状态，北向读到的就是"退出/远方/0"，而不是点不存在
        self.inner.tick().await;
        *self.handles.lock() = vec![
            tokio::spawn(run_tick(self.inner.clone(), self.stop_rx.clone())),
            tokio::spawn(run_downlink(self.inner.clone(), rx, self.stop_rx.clone())),
        ];
        self.state.store(self.id(), LifecycleState::Running);
        tracing::info!(
            "[{}] 协调控制器已启动, 成员: {}",
            self.id(),
            self.inner
                .members
                .iter()
                .map(|m| m.dev.as_str())
                .collect::<Vec<_>>()
                .join(",")
        );
        Ok(())
    }

    async fn stop(&self) -> Result<(), DeviceError> {
        let _ = self.stop_tx.send(true);
        if self.state.load() == LifecycleState::Stopped {
            return Ok(());
        }
        data_center().detach_downlink(self.id());
        let handles: Vec<JoinHandle<()>> = self.handles.lock().drain(..).collect();
        for mut h in handles {
            tokio::select! {
                _ = time::sleep(Duration::from_secs(3)) => h.abort(),
                _ = &mut h => {}
            }
        }
        self.state.store(self.id(), LifecycleState::Stopped);
        Ok(())
    }

    fn state(&self) -> LifecycleState {
        self.state.load()
    }
}

impl Identifiable for Coordinator {
    fn id(&self) -> &str {
        &self.inner.id
    }
}

impl Executable for Coordinator {}

#[cfg(test)]
mod tests {
    use super::*;
    use collector_core::dispatch::northbound_allows;
    use tokio::time::{Instant, sleep, timeout};

    const ID: &str = "co-it";
    const A: &str = "co-it-a";
    const B: &str = "co-it-b";

    fn member(dev: &str) -> CoordinatorMember {
        CoordinatorMember {
            dev: dev.into(),
            rated_kva: 500.0,
            p_set: 1,
            q_set: 2,
            p_actual: 3,
            q_actual: 4,
            p_upper: 11,
            p_lower: 12,
            q_upper: 13,
            q_lower: 14,
        }
    }

    fn dp(id: PointId, v: f64) -> DataPoint {
        P_SET.point(Val::F64(v)).with_id(id)
    }

    trait WithId {
        fn with_id(self, id: PointId) -> Self;
    }
    impl WithId for DataPoint {
        fn with_id(mut self, id: PointId) -> Self {
            self.id = id;
            self
        }
    }

    fn ingest_member(dev: &str, p_actual: f64) {
        data_center().ingest(
            dev,
            vec![
                dp(3, p_actual),
                dp(4, 0.0),
                dp(11, 500.0),
                dp(12, -500.0),
                dp(13, 500.0),
                dp(14, -500.0),
            ],
        );
    }

    fn value(dev: &str, id: PointId) -> Option<Val> {
        data_center().read(dev, id).map(|p| p.value)
    }

    async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "等待超时: {what}");
            sleep(Duration::from_millis(20)).await;
        }
    }

    /// 取成员收到的下一批下发，返回 (点 id -> 值)。
    async fn next_down(rx: &mut mpsc::Receiver<Vec<DownDataPoint>>) -> Vec<(PointId, Val)> {
        let batch = timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("等待下发超时")
            .expect("通道关闭");
        batch
            .into_iter()
            .map(|p| match p.point {
                PointRef::Id(id) => (id, p.value),
                other => panic!("意外的点引用 {other:?}"),
            })
            .collect()
    }

    async fn set(points: Vec<DownDataPoint>) {
        data_center().dispatch(ID, points).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn agc_flow() {
        let center = data_center();
        ingest_member(A, 10.0);
        ingest_member(B, 20.0);
        let (tx_a, mut rx_a) = mpsc::channel(16);
        let (tx_b, mut rx_b) = mpsc::channel(16);
        center.attach_downlink(A, tx_a).unwrap();
        center.attach_downlink(B, tx_b).unwrap();

        let program = CoordinatorProgram {
            enable: true,
            id: ID.into(),
            tick_ms: 50,
            upstream_timeout_ms: 0,
            p_deadband: 1.0,
            q_deadband: 1.0,
            members: vec![member(A), member(B)],
        };
        let mut co = Coordinator::new(&program).unwrap();
        co.init().unwrap();
        co.start().await.unwrap();

        // 1. 初始状态：退出、远方、设定值 0；总功率 = A+B 实测之和；成员都在线
        wait_until("初始上送", || {
            value(ID, TOTAL_P.id) == Some(Val::F64(30.0))
        })
        .await;
        assert_eq!(value(ID, AGC_ENABLE.id), Some(Val::U8(0)));
        assert_eq!(value(ID, AGC_REMOTE.id), Some(Val::U8(1)));
        assert_eq!(value(ID, P_SET.id), Some(Val::F64(0.0)));
        assert_eq!(value(ID, COMM_STATUS_ID), Some(Val::U8(0)));
        assert_eq!(value(ID, P_UPPER.id), Some(Val::F64(1000.0)));

        // 2. 门禁：退出时拒绝总设定值，但开关始终允许
        let p_cmd = || DownDataPoint::by_id(P_SET.id, Val::F64(200.0));
        assert!(!northbound_allows(ID, &p_cmd()));
        assert!(northbound_allows(
            ID,
            &DownDataPoint::by_id(AGC_ENABLE.id, Val::U8(1))
        ));
        assert!(northbound_allows(
            ID,
            &DownDataPoint::by_id(AGC_REMOTE.id, Val::U8(0))
        ));
        // 其他设备不受影响
        assert!(northbound_allows("other-dev", &p_cmd()));

        // 3. 投入后允许；设定 200 -> A、B 各 100
        set(vec![DownDataPoint::by_id(AGC_ENABLE.id, Val::U8(1))]).await;
        wait_until("投入", || value(ID, AGC_ENABLE.id) == Some(Val::U8(1))).await;
        assert!(northbound_allows(ID, &p_cmd()));
        // 投入时设定值从 0 开始，成员先收到一次 0
        assert_eq!(next_down(&mut rx_a).await, vec![(1, Val::F64(0.0))]);
        assert_eq!(next_down(&mut rx_b).await, vec![(1, Val::F64(0.0))]);
        set(vec![p_cmd()]).await;
        assert_eq!(next_down(&mut rx_a).await, vec![(1, Val::F64(100.0))]);
        assert_eq!(next_down(&mut rx_b).await, vec![(1, Val::F64(100.0))]);
        wait_until("设定值回显", || {
            value(ID, P_SET.id) == Some(Val::F64(200.0))
        })
        .await;

        // 4. 就地：北向设定被拒绝，但可以再切回远方
        set(vec![DownDataPoint::by_id(AGC_REMOTE.id, Val::U8(0))]).await;
        wait_until("就地", || value(ID, AGC_REMOTE.id) == Some(Val::U8(0))).await;
        assert!(!northbound_allows(ID, &p_cmd()));
        set(vec![DownDataPoint::by_id(AGC_REMOTE.id, Val::U8(1))]).await;
        wait_until("远方", || value(ID, AGC_REMOTE.id) == Some(Val::U8(1))).await;
        assert!(northbound_allows(ID, &p_cmd()));

        // 5. B 通讯故障：设定值保持，A 接手全部 200；总通讯状态置故障
        center.ingest(B, vec![dp(COMM_STATUS_ID, 1.0)]);
        assert_eq!(next_down(&mut rx_a).await, vec![(1, Val::F64(200.0))]);
        wait_until("故障", || value(ID, COMM_STATUS_ID) == Some(Val::U8(1))).await;
        assert_eq!(
            value(ID, TOTAL_P.id),
            Some(Val::F64(10.0)),
            "离线成员不计入总功率"
        );
        // B 恢复：重新分配
        center.ingest(B, vec![dp(COMM_STATUS_ID, 0.0)]);
        assert_eq!(next_down(&mut rx_a).await, vec![(1, Val::F64(100.0))]);
        assert_eq!(next_down(&mut rx_b).await, vec![(1, Val::F64(100.0))]);

        // 6. 退出：设定值和各成员都回到 0
        set(vec![DownDataPoint::by_id(AGC_ENABLE.id, Val::U8(0))]).await;
        assert_eq!(next_down(&mut rx_a).await, vec![(1, Val::F64(0.0))]);
        assert_eq!(next_down(&mut rx_b).await, vec![(1, Val::F64(0.0))]);
        wait_until("设定清零", || {
            value(ID, P_SET.id) == Some(Val::F64(0.0))
        })
        .await;
        assert!(!northbound_allows(ID, &p_cmd()));

        co.stop().await.unwrap();
        center.detach_downlink(A);
        center.detach_downlink(B);
    }

    #[test]
    fn rejects_bad_config() {
        let base = CoordinatorProgram {
            members: vec![member("x")],
            ..Default::default()
        };
        assert!(Coordinator::new(&base).is_ok());
        let empty = CoordinatorProgram::default();
        assert!(matches!(
            Coordinator::new(&empty),
            Err(CoordinatorError::NoMembers)
        ));
        let mut bad = base.clone();
        bad.members[0].rated_kva = 0.0;
        assert!(matches!(
            Coordinator::new(&bad),
            Err(CoordinatorError::BadRated(_))
        ));
        let mut bad = base;
        bad.tick_ms = 0;
        assert!(matches!(
            Coordinator::new(&bad),
            Err(CoordinatorError::BadTick)
        ));
    }
}
