use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use collector_iec60870::{
    ApciParameters, Asdu, Client, ConnectionEvent, DoublePointValue, InformationObjectValue,
};
use tokio::sync::{mpsc, watch};
use tokio::time;
use tracing::{debug, info, warn};

use crate::center::data_center;
use crate::config::iec104_conf::{Iec104Config, PointKind, build_monitor_index};
use crate::core::point::{DataPoint, DownDataPoint, Val};
use crate::dev::dev_config::Iec104DeviceConfig;
use crate::dev::{LifecycleState, state::SharedState};

use super::backoff::Backoff;
use super::downlink::{ControlIndex, send_command};

/// 连接保持超过该时长才视为"稳定"并重置重连退避，防止对端反复秒断时高频重连。
const STABLE_AFTER: Duration = Duration::from_secs(30);

/// 连接健康检查间隔。
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(1);

pub(super) struct Iec60870Runner {
    pub(super) id: String,
    pub(super) config: Iec104DeviceConfig,
    pub(super) configs: Vec<Iec104Config>,
    pub(super) state: SharedState,
    pub(super) stop_rx: watch::Receiver<bool>,
    pub(super) rx: mpsc::Receiver<Vec<DownDataPoint>>,
}

impl Iec60870Runner {
    /// 上报通讯故障位：false = 有通讯，true = 无通讯。
    fn set_comm_fault(&self, fault: bool) {
        data_center().ingest(
            &self.id,
            vec![DataPoint {
                id: crate::config::iec104_conf::COMM_STATUS_ID,
                name: "通讯状态".into(),
                value: Val::U8(fault as u8),
                key: "communicationStatus".into(),
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

    /// 新建连接对象、注册回调并（在阻塞线程中）完成 TCP 连接。
    /// lib60870 要求回调与 APCI 参数在 connect 之前设置，且重连时复用对象行为未定义，
    /// 因此每次重连都新建一个 `Client`。
    async fn connect(
        &self,
        monitor: &Arc<HashMap<(i32, i32), Iec104Config>>,
        events_tx: mpsc::UnboundedSender<ConnectionEvent>,
    ) -> anyhow::Result<Client> {
        let mut client = Client::new(&self.config.ip, self.config.port)?;
        client.set_apci_parameters(ApciParameters {
            k: 12,
            w: 8,
            t0: self.config.timeout.as_secs().max(1) as i32,
            t1: 15,
            t2: 10,
            t3: 20,
        });
        let dev_id = self.id.clone();
        let monitor = Arc::clone(monitor);
        client.set_asdu_received_handler(move |_addr, asdu| handle_asdu(&dev_id, &monitor, asdu));
        client.set_connection_event_handler(move |event| {
            let _ = events_tx.send(event);
        });
        tokio::task::spawn_blocking(move || client.connect().map(|_| client))
            .await?
            .map_err(Into::into)
    }

    /// 对点表中出现的每个公共地址发送总召唤。
    fn interrogate(&self, client: &mut Client, cas: &[i32]) -> anyhow::Result<()> {
        for &ca in cas {
            client.send_interrogation_command(ca)?;
        }
        debug!("[{}] 已发送总召唤: CA={:?}", self.id, cas);
        Ok(())
    }

    async fn run_connected(
        &mut self,
        client: &mut Client,
        events_rx: &mut mpsc::UnboundedReceiver<ConnectionEvent>,
        stop_rx: &mut watch::Receiver<bool>,
        cas: &[i32],
        control: &ControlIndex,
    ) -> anyhow::Result<()> {
        client.start_data_transfer();
        self.interrogate(client, cas)?;
        self.state.store(&self.id, LifecycleState::Running);
        self.set_comm_fault(false);

        let mut health = time::interval(HEALTH_CHECK_INTERVAL);
        // 间隔为0表示只在连接建立后召唤一次
        let mut cycle = (!self.config.interval.is_zero()).then(|| {
            time::interval_at(
                time::Instant::now() + self.config.interval,
                self.config.interval,
            )
        });

        loop {
            tokio::select! {
                changed = stop_rx.changed() => {
                    if changed.is_err() || Self::stop_requested(stop_rx) {
                        return Ok(());
                    }
                }
                event = events_rx.recv() => {
                    match event {
                        Some(ConnectionEvent::Closed) => return Err(anyhow!("连接被关闭")),
                        Some(ConnectionEvent::Failed) => return Err(anyhow!("连接异常")),
                        Some(other) => debug!("[{}] 连接事件: {:?}", self.id, other),
                        None => return Err(anyhow!("连接事件通道已关闭")),
                    }
                }
                _ = health.tick() => {
                    if !client.is_connected() {
                        return Err(anyhow!("连接已断开"));
                    }
                }
                _ = async {
                    match cycle.as_mut() {
                        Some(c) => { c.tick().await; }
                        None => std::future::pending::<()>().await,
                    }
                } => {
                    self.interrogate(client, cas)?;
                }
                msg = self.rx.recv() => {
                    let Some(entries) = msg else {
                        self.state.store(&self.id, LifecycleState::Stopped);
                        return Ok(());
                    };
                    let items: Vec<String> = entries.iter().map(|e| format!("{:?}: {}", e.point, e.value)).collect();
                    info!("[{}] ↓: {}", self.id, items.join(", "));
                    for entry in &entries {
                        let Some(cfg) = control.resolve(&entry.point) else {
                            warn!("[{}] 未找到控制点配置, 忽略IEC104下发: {:?}", self.id, entry.point);
                            continue;
                        };
                        if let Err(err) = send_command(client, cfg, &entry.value) {
                            warn!("[{}] IEC104下发失败: {:#}", self.id, err);
                        }
                    }
                }
            }
        }
    }

    pub(super) async fn run(mut self) {
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        let mut stop_rx = self.stop_rx.clone();
        let monitor = Arc::new(build_monitor_index(&self.configs));
        let control = ControlIndex::build(&self.configs);
        let mut cas: Vec<i32> = monitor.keys().map(|(ca, _)| *ca).collect();
        cas.sort_unstable();
        cas.dedup();
        if cas.is_empty() {
            warn!("[{}] 点表中没有启用的遥信/遥测点, 不会发送总召唤", self.id);
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
            match self.connect(&monitor, events_tx).await {
                Ok(mut client) => {
                    self.state.store(&self.id, LifecycleState::Connected);
                    let connected_at = time::Instant::now();
                    let result = self
                        .run_connected(&mut client, &mut events_rx, &mut stop_rx, &cas, &control)
                        .await;
                    // 关闭连接会等待底层线程退出，放到阻塞线程里避免卡住运行时
                    let _ = tokio::task::spawn_blocking(move || drop(client)).await;
                    self.set_comm_fault(true);
                    if connected_at.elapsed() >= STABLE_AFTER {
                        backoff.reset();
                    }
                    if let Err(err) = result {
                        self.state.store(&self.id, LifecycleState::Failed);
                        warn!("[{}] IEC104连接中断，准备重连: {:#}", self.id, err);
                    } else if self.state.load() == LifecycleState::Stopped {
                        // 下行通道关闭，设备已被注销
                        return;
                    }
                }
                Err(err) => {
                    self.state.store(&self.id, LifecycleState::Failed);
                    warn!("[{}] IEC104连接失败，准备重连: {:#}", self.id, err);
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

/// 处理对端上送的 ASDU：按 (CA, IOA) 匹配点表，转换后写入数据中心。
/// 在 lib60870 的接收线程中被调用，只做同步、非阻塞的工作。
fn handle_asdu(dev_id: &str, monitor: &HashMap<(i32, i32), Iec104Config>, asdu: Asdu) -> bool {
    if asdu.is_negative {
        warn!(
            "[{}] 收到否定确认: type={} cot={} ca={}",
            dev_id, asdu.type_id, asdu.cot, asdu.ca
        );
    }
    let points: Vec<DataPoint> = asdu
        .elements
        .iter()
        .filter_map(|el| {
            let cfg = monitor.get(&(asdu.ca, el.ioa))?;
            let value = decode_value(cfg, &el.value)?;
            Some(cfg.to_data_point(value))
        })
        .collect();
    if !points.is_empty() {
        data_center().ingest(dev_id, points);
    }
    true
}

/// f32 直接转 f64 会带出二进制误差（如 220.1 变成 220.10000610...），
/// 先按最短十进制表示还原再解析，避免展示和比较时出现噪声。
fn f32_to_f64(v: f32) -> f64 {
    v.to_string().parse().unwrap_or_else(|_| f64::from(v))
}

/// 把信息对象的值转换为点值。点类型与 ASDU 类型不匹配（如遥信点收到遥测）时丢弃。
fn decode_value(cfg: &Iec104Config, value: &InformationObjectValue) -> Option<Val> {
    let measured = |raw: f64| Some(Val::F64(raw * cfg.scale + cfg.offset));
    match (cfg.kind, value) {
        (PointKind::Yx, InformationObjectValue::SinglePoint { value, .. }) => {
            Some(Val::U8(*value as u8))
        }
        (PointKind::Yx, InformationObjectValue::DoublePoint { value, .. }) => {
            Some(Val::U8(match value {
                DoublePointValue::Intermediate => 0,
                DoublePointValue::Off => 1,
                DoublePointValue::On => 2,
                DoublePointValue::Indeterminate => 3,
            }))
        }
        (PointKind::Yc, InformationObjectValue::MeasuredNormalized { value, .. })
        | (PointKind::Yc, InformationObjectValue::MeasuredShort { value, .. }) => {
            measured(f32_to_f64(*value))
        }
        (PointKind::Yc, InformationObjectValue::MeasuredScaled { value, .. }) => {
            measured(f64::from(*value))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(kind: PointKind, scale: f64, offset: f64) -> Iec104Config {
        Iec104Config {
            id: 1,
            key: "k".into(),
            name: "n".into(),
            ca: 1,
            ioa: 100,
            kind,
            scale,
            offset,
            unit: None,
            enable: true,
            trans: None,
            status_words: None,
            warn_bits: None,
            level: None,
        }
    }

    #[test]
    fn yx_single_and_double() {
        let c = cfg(PointKind::Yx, 1.0, 0.0);
        let sp = InformationObjectValue::SinglePoint {
            value: true,
            quality: 0,
        };
        assert_eq!(decode_value(&c, &sp), Some(Val::U8(1)));
        let dp = InformationObjectValue::DoublePoint {
            value: DoublePointValue::On,
            quality: 0,
        };
        assert_eq!(decode_value(&c, &dp), Some(Val::U8(2)));
    }

    #[test]
    fn yc_applies_scale_and_offset() {
        let c = cfg(PointKind::Yc, 0.1, 5.0);
        let v = InformationObjectValue::MeasuredScaled {
            value: 100,
            quality: 0,
        };
        assert_eq!(decode_value(&c, &v), Some(Val::F64(15.0)));
    }

    #[test]
    fn mismatched_kind_is_dropped() {
        let c = cfg(PointKind::Yx, 1.0, 0.0);
        let v = InformationObjectValue::MeasuredShort {
            value: 1.0,
            quality: 0,
        };
        assert_eq!(decode_value(&c, &v), None);
        assert_eq!(
            decode_value(&c, &InformationObjectValue::Unsupported { type_id: 45 }),
            None
        );
    }

    #[test]
    fn short_float_has_no_binary_noise() {
        let c = cfg(PointKind::Yc, 1.0, 0.0);
        let v = InformationObjectValue::MeasuredShort {
            value: 220.1,
            quality: 0,
        };
        assert_eq!(decode_value(&c, &v), Some(Val::F64(220.1)));
    }

    #[test]
    fn point_kind_parse() {
        assert_eq!(
            PointKind::try_from(" YK_Double ").unwrap(),
            PointKind::YkDouble
        );
        assert!(PointKind::try_from("bogus").is_err());
    }
}
