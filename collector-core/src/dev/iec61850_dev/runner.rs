use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use collector_iec61850::{ConnectOptions, Connection, ConnectionState, Error};
use tokio::sync::{mpsc, watch};
use tokio::time;
use tracing::{debug, info, warn};

use crate::center::data_center;
use crate::config::iec61850_conf::{COMM_STATUS_ID, Iec61850Configs};
use crate::core::point::{DataPoint, DownDataPoint, Val};
use crate::dev::dev_config::Iec61850DeviceConfig;
use crate::dev::{LifecycleState, state::SharedState};

use super::backoff::Backoff;
use super::downlink::{ControlIndex, Verification, command_value, confirm, describe, execute};
use super::session::Session;

/// 连接保持超过该时长才视为"稳定"并重置重连退避，防止对端反复秒断时高频重连。
const STABLE_AFTER: Duration = Duration::from_secs(30);

/// 连接健康检查间隔。
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(1);

pub(super) struct Iec61850Runner {
    pub(super) id: String,
    pub(super) config: Iec61850DeviceConfig,
    pub(super) configs: Iec61850Configs,
    pub(super) state: SharedState,
    pub(super) stop_rx: watch::Receiver<bool>,
    pub(super) rx: mpsc::Receiver<Vec<DownDataPoint>>,
}

/// 在阻塞线程里执行同步的 MMS 调用。等待期间收到停止请求就放弃等待返回 `Ok(None)`
/// (任务本身会自己结束); 任务 panic 返回 `Err`。
async fn run_blocking<T: Send + 'static>(
    stop_rx: &mut watch::Receiver<bool>,
    f: impl FnOnce() -> T + Send + 'static,
) -> anyhow::Result<Option<T>> {
    let mut handle = tokio::task::spawn_blocking(f);
    loop {
        tokio::select! {
            result = &mut handle => return Ok(Some(result?)),
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    return Ok(None);
                }
            }
        }
    }
}

impl Iec61850Runner {
    /// 上报通讯故障位：false = 有通讯，true = 无通讯。
    fn set_comm_fault(&self, fault: bool) {
        data_center().ingest(
            &self.id,
            vec![DataPoint {
                id: COMM_STATUS_ID,
                name: "通讯状态",
                value: Val::U8(fault as u8),
                key: "communicationStatus",
                translator: None,
                bits: None,
                words: None,
                unit: None,
                level: None,
            }],
        );
    }

    fn stop_requested(stop_rx: &watch::Receiver<bool>) -> bool {
        *stop_rx.borrow()
    }

    /// 新建连接并（在阻塞线程中）完成 MMS 关联。每次重连都新建一个 `Connection`。
    async fn connect(
        &self,
        events_tx: mpsc::UnboundedSender<ConnectionState>,
    ) -> anyhow::Result<Arc<Connection>> {
        let millis = u32::try_from(self.config.timeout.as_millis()).unwrap_or(u32::MAX);
        let options = ConnectOptions {
            connect_timeout_ms: millis,
            request_timeout_ms: millis,
            iso: self.config.iso.clone(),
        };
        let (ip, port) = (self.config.ip.clone(), self.config.port);
        tokio::task::spawn_blocking(move || -> Result<Arc<Connection>, Error> {
            let conn = Connection::new(options)?;
            conn.on_state_changed(move |state| {
                let _ = events_tx.send(state);
            });
            conn.connect(&ip, port)?;
            Ok(Arc::new(conn))
        })
        .await?
        .map_err(Into::into)
    }

    /// 建立会话(订阅报告等)并服务到连接断开或收到停止请求。
    ///
    /// `conn` 的所有权交给会话: `Connection` 销毁时要等库的接收线程退出, 不能在异步线程里
    /// 释放, 所以最后一个引用只会在阻塞线程里落下。
    async fn run_connected(
        &mut self,
        conn: Arc<Connection>,
        events_rx: &mut mpsc::UnboundedReceiver<ConnectionState>,
        stop_rx: &mut watch::Receiver<bool>,
        control: &ControlIndex,
    ) -> anyhow::Result<()> {
        let (id, configs, interval) = (self.id.clone(), self.configs.clone(), self.config.interval);
        let established = run_blocking(stop_rx, move || Session::establish(conn, &id, &configs, interval)).await?;
        let Some(session) = established else {
            return Ok(());
        };
        let session = Arc::new(session?);

        let result = self.serve(&session, events_rx, stop_rx, control).await;
        // 释放会话会销毁连接并等待底层线程退出, 放到阻塞线程里避免卡住运行时
        let _ = tokio::task::spawn_blocking(move || drop(session)).await;
        result
    }

    async fn serve(
        &mut self,
        session: &Arc<Session>,
        events_rx: &mut mpsc::UnboundedReceiver<ConnectionState>,
        stop_rx: &mut watch::Receiver<bool>,
        control: &ControlIndex,
    ) -> anyhow::Result<()> {
        self.state.store(&self.id, LifecycleState::Running);
        self.set_comm_fault(false);
        info!("[{}] 已建立会话: 订阅 {} 个报告控制块", self.id, session.subscribed);

        // 轮询类来源先读一遍(报告类来源由使能时的总召负责)
        let s = session.clone();
        match run_blocking(stop_rx, move || s.initial_poll()).await? {
            None => return Ok(()),
            Some(result) => result?,
        }

        let mut health = time::interval(HEALTH_CHECK_INTERVAL);
        let start = time::Instant::now();
        let mut due: Vec<time::Instant> = session.schedule.iter().map(|s| start + s.every).collect();

        loop {
            let next_due = due.iter().copied().min();
            tokio::select! {
                changed = stop_rx.changed() => {
                    if changed.is_err() || Self::stop_requested(stop_rx) {
                        return Ok(());
                    }
                }
                event = events_rx.recv() => {
                    match event {
                        Some(ConnectionState::Closed) => return Err(anyhow!("连接被关闭")),
                        Some(other) => debug!("[{}] 连接状态: {:?}", self.id, other),
                        None => return Err(anyhow!("连接事件通道已关闭")),
                    }
                }
                _ = health.tick() => {
                    if !session.conn.is_connected() {
                        return Err(anyhow!("连接已断开"));
                    }
                }
                _ = async {
                    match next_due {
                        Some(at) => time::sleep_until(at).await,
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    let now = time::Instant::now();
                    let jobs: Vec<usize> = (0..due.len()).filter(|&i| due[i] <= now).collect();
                    for &i in &jobs {
                        due[i] = now + session.schedule[i].every;
                    }
                    let s = session.clone();
                    let outcome = run_blocking(stop_rx, move || -> Result<(), Error> {
                        jobs.into_iter().try_for_each(|i| s.run_job(&s.schedule[i].job))
                    })
                    .await?;
                    match outcome {
                        None => return Ok(()),
                        Some(result) => result?,
                    }
                }
                msg = self.rx.recv() => {
                    let Some(entries) = msg else {
                        self.state.store(&self.id, LifecycleState::Stopped);
                        return Ok(());
                    };
                    let items: Vec<String> = entries.iter().map(|e| format!("{:?}: {}", e.point, e.value)).collect();
                    info!("[{}] ↓: {}", self.id, items.join(", "));
                    for entry in &entries {
                        if self.send_one(session, control, entry, stop_rx).await? {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }

    /// 下发一条命令。返回 `Ok(true)` 表示下发期间收到了停止请求; 连接丢失返回 `Err`。
    async fn send_one(
        &self,
        session: &Arc<Session>,
        control: &ControlIndex,
        entry: &DownDataPoint,
        stop_rx: &mut watch::Receiver<bool>,
    ) -> anyhow::Result<bool> {
        let Some(cfg) = control.resolve(&entry.point) else {
            warn!("[{}] 未找到控制点配置, 忽略IEC61850下发: {:?}", self.id, entry.point);
            return Ok(false);
        };
        let raw = match command_value(cfg, &entry.value) {
            Ok(raw) => raw,
            Err(err) => {
                warn!("[{}] IEC61850下发失败 {}: {:#}", self.id, cfg.key, err);
                return Ok(false);
            }
        };
        let feedback = control.feedback_of(cfg).cloned();
        let (s, c) = (session.clone(), cfg.clone());
        let outcome = run_blocking(stop_rx, move || {
            // 先读下发前的回读值, 之后才能判断它有没有变化
            let before = feedback.as_ref().and_then(|f| s.read_point(f));
            execute(&s.conn, &c, raw).map(|()| (before, feedback))
        })
        .await?;
        match outcome {
            None => Ok(true),
            Some(Err(err)) if err.is_connection_lost() => Err(err.into()),
            Some(Err(err)) => {
                warn!("[{}] IEC61850下发失败 {}: {}", self.id, cfg.key, describe(&err));
                Ok(false)
            }
            Some(Ok((before, feedback))) => {
                let verification = feedback
                    .and_then(|f| Verification::new(&self.id, cfg, &f, before, &entry.value));
                match verification {
                    Some(v) => {
                        // 确认可能要等几秒, 放到独立的阻塞任务里, 不挡住后面的命令
                        let s = session.clone();
                        tokio::task::spawn_blocking(move || confirm(&s, &v));
                    }
                    None => info!(
                        "[{}] 命令 {} 应答成功, 但没有可用的回读点, 无法确认设备是否执行",
                        self.id, cfg.key
                    ),
                }
                Ok(false)
            }
        }
    }

    pub(super) async fn run(mut self) {
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        let mut stop_rx = self.stop_rx.clone();
        let control = ControlIndex::build(&self.configs);
        if !self.configs.points.iter().any(|p| p.enable && p.kind.is_monitor()) {
            warn!("[{}] 点表中没有启用的监视点", self.id);
        }

        loop {
            if Self::stop_requested(&stop_rx) {
                self.state.store(&self.id, LifecycleState::Stopped);
                self.set_comm_fault(true);
                return;
            }

            self.state.store(&self.id, LifecycleState::Connecting);
            self.set_comm_fault(true);

            let (events_tx, mut events_rx) = mpsc::unbounded_channel();
            match self.connect(events_tx).await {
                Ok(conn) => {
                    self.state.store(&self.id, LifecycleState::Connected);
                    let connected_at = time::Instant::now();
                    let result = self
                        .run_connected(conn, &mut events_rx, &mut stop_rx, &control)
                        .await;
                    self.set_comm_fault(true);
                    if connected_at.elapsed() >= STABLE_AFTER {
                        backoff.reset();
                    }
                    if let Err(err) = result {
                        self.state.store(&self.id, LifecycleState::Failed);
                        warn!("[{}] IEC61850连接中断，准备重连: {:#}", self.id, err);
                    } else if self.state.load() == LifecycleState::Stopped {
                        // 下行通道关闭，设备已被注销
                        return;
                    }
                }
                Err(err) => {
                    self.state.store(&self.id, LifecycleState::Failed);
                    warn!("[{}] IEC61850连接失败，准备重连: {:#}", self.id, err);
                }
            }

            if Self::stop_requested(&stop_rx) {
                continue;
            }
            let delay = backoff.next_delay();
            tokio::select! {
                _ = time::sleep(delay) => {}
                _ = stop_rx.changed() => {}
            }
        }
    }
}
