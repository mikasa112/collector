use std::collections::HashMap;
use std::io;
use std::time::Duration;

use collector_dlt645::{Client, ClientConfig, MeterAddress, MeterError};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::watch;
use tokio::time::{self, Instant};
use tokio_serial::{DataBits, Parity, SerialStream, StopBits};
use tracing::{error, warn};

use crate::center::data_center;
use crate::config::dlt645_conf::{COMM_STATUS_ID, Dlt645Config, Dlt645Configs};
use crate::core::point::{DataPoint, Val};
use crate::dev::dev_config::Dlt645DeviceConfig;
use crate::dev::{LifecycleState, state::SharedState};

use super::backoff::Backoff;

/// 某块电表连续无响应(超时/坏帧/串扰)达到该次数即视为离线
const MAX_READ_FAILURES: u32 = 3;
/// 帧前的 0xFE 唤醒字节个数
const WAKEUP_BYTES: usize = 4;

/// 轮询计划里的一项：已确定电表地址的点。
#[derive(Debug, Clone)]
pub(super) struct PollItem {
    pub(super) address: MeterAddress,
    pub(super) cfg: Dlt645Config,
}

/// 由点表和设备默认地址生成轮询计划。
///
/// 按行容错：停用的点、既没有点表地址也没有设备默认地址的点、
/// 同一块电表上重复读取同一个 DI 的点，会记录错误并被跳过。
pub(super) fn build_plan(
    configs: Dlt645Configs,
    default_address: Option<MeterAddress>,
) -> Vec<PollItem> {
    let mut plan: Vec<PollItem> = Vec::new();
    for cfg in configs.into_iter().filter(|c| c.enable) {
        let Some(address) = cfg.address.or(default_address) else {
            error!("忽略DLT645点 {}: 点表和设备配置都没有电表地址", cfg.key);
            continue;
        };
        if let Some(prev) = plan
            .iter()
            .find(|p| p.address == address && p.cfg.spec.di == cfg.spec.di)
        {
            error!(
                "忽略DLT645点 {}: 与 {} 读取同一个电表同一个DI {:08X}",
                cfg.key,
                prev.cfg.key,
                cfg.spec.di.as_u32()
            );
            continue;
        }
        plan.push(PollItem { address, cfg });
    }
    plan
}

/// 每块电表的健康状态，用来决定设备级通讯故障位。
///
/// 设备级故障 = 所有电表都已连续失败达到阈值；任一电表有响应即恢复。
/// 初始为故障，直到第一次收到电表的响应。
pub(super) struct Health {
    streaks: HashMap<MeterAddress, u32>,
    fault: bool,
}

impl Health {
    pub(super) fn new(plan: &[PollItem]) -> Self {
        Self {
            streaks: plan.iter().map(|p| (p.address, 0)).collect(),
            fault: true,
        }
    }

    /// 记录一次结果；设备级故障位发生变化时返回新值。
    pub(super) fn record(&mut self, address: MeterAddress, alive: bool) -> Option<bool> {
        let streak = self.streaks.entry(address).or_insert(0);
        if alive {
            *streak = 0;
        } else {
            *streak = streak.saturating_add(1);
        }
        let fault = self.streaks.values().all(|s| *s >= MAX_READ_FAILURES);
        let changed = fault != self.fault && (alive || fault);
        if changed {
            self.fault = fault;
            Some(fault)
        } else {
            None
        }
    }
}

/// 电表给出了有效应答(哪怕是异常应答或数据无法解析)就说明它在线；
/// 超时、坏帧、地址不符(串扰)才算失败。
fn is_alive(err: &MeterError) -> bool {
    matches!(
        err,
        MeterError::MeterException(_)
            | MeterError::Value(_)
            | MeterError::IdentifierMismatch
            | MeterError::UnexpectedControl(_)
            | MeterError::MissingIdentifier
    )
}

enum Exit {
    Stop,
    /// 串口 I/O 出错(比如被拔掉)，需要重新打开
    Reconnect,
}

pub(super) struct Dlt645Runner {
    pub(super) id: String,
    pub(super) config: Dlt645DeviceConfig,
    pub(super) plan: Vec<PollItem>,
    pub(super) state: SharedState,
    pub(super) stop_rx: watch::Receiver<bool>,
}

/// 等待停止信号；发送端被丢弃也视为停止。
async fn wait_stop(rx: &mut watch::Receiver<bool>) {
    loop {
        if *rx.borrow() {
            return;
        }
        if rx.changed().await.is_err() {
            return;
        }
    }
}

impl Dlt645Runner {
    /// 上报通讯故障位：false = 有通讯，true = 无通讯。
    fn set_comm_fault(&self, fault: bool) {
        data_center().ingest(
            &self.id,
            vec![DataPoint {
                id: COMM_STATUS_ID,
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

    fn open(&self) -> io::Result<SerialStream> {
        let cfg = &self.config;
        let builder = tokio_serial::new(cfg.serial_tty.as_str(), cfg.baudrate)
            .data_bits(match cfg.data_bits {
                5 => DataBits::Five,
                6 => DataBits::Six,
                7 => DataBits::Seven,
                _ => DataBits::Eight,
            })
            .parity(match cfg.parity.to_ascii_uppercase().as_str() {
                "E" | "EVEN" => Parity::Even,
                "O" | "ODD" => Parity::Odd,
                _ => Parity::None,
            })
            .stop_bits(match cfg.stop_bits {
                2 => StopBits::Two,
                _ => StopBits::One,
            });
        Ok(SerialStream::open(&builder)?)
    }

    /// 相邻两次请求的最小间隔：`interval` 按点数均分(使一整轮耗时贴近配置值)，
    /// 但不低于 `request_interval`(总线空闲时间，下限 1ms)。
    fn pace(&self) -> Duration {
        let floor = self.config.request_interval.max(Duration::from_millis(1));
        if self.config.interval.is_zero() {
            return floor;
        }
        (self.config.interval / self.plan.len().max(1) as u32).max(floor)
    }

    /// 轮询主循环：每次读一个点，读完等够 `pace` 再读下一个，直到停止或串口 I/O 出错。
    async fn run_connected<S>(
        &self,
        client: &mut Client<S>,
        stop_rx: &mut watch::Receiver<bool>,
    ) -> Exit
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.state.store(&self.id, LifecycleState::Running);
        let pace = self.pace();
        let floor = self.config.request_interval.max(Duration::from_millis(1));
        let mut health = Health::new(&self.plan);
        let mut index = 0;
        loop {
            let item = &self.plan[index];
            index = (index + 1) % self.plan.len();
            let started = Instant::now();
            let result = tokio::select! {
                biased;
                _ = wait_stop(stop_rx) => return Exit::Stop,
                r = client.read_value(item.address, &item.cfg.spec) => r,
            };
            let alive = match result {
                Ok(decoded) => {
                    let point = item.cfg.to_data_point(item.cfg.to_val(decoded));
                    data_center().ingest(&self.id, vec![point]);
                    true
                }
                Err(MeterError::Io(err)) => {
                    warn!("[{}] 串口读写出错, 准备重新打开: {}", self.id, err);
                    return Exit::Reconnect;
                }
                Err(err) => {
                    warn!(
                        "[{}] 读取 {} (电表{:?} DI {:08X}) 失败: {}",
                        self.id,
                        item.cfg.key,
                        item.address.to_digits().unwrap_or_default(),
                        item.cfg.spec.di.as_u32(),
                        err
                    );
                    is_alive(&err)
                }
            };
            if let Some(fault) = health.record(item.address, alive) {
                self.set_comm_fault(fault);
            }
            let wait = pace.saturating_sub(started.elapsed()).max(floor);
            tokio::select! {
                biased;
                _ = wait_stop(stop_rx) => return Exit::Stop,
                _ = time::sleep(wait) => {}
            }
        }
    }

    pub(super) async fn run(self) {
        let mut stop_rx = self.stop_rx.clone();
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        loop {
            if *stop_rx.borrow() {
                self.state.store(&self.id, LifecycleState::Stopped);
                self.set_comm_fault(true);
                return;
            }
            self.state.store(&self.id, LifecycleState::Connecting);
            self.set_comm_fault(true);

            match self.open() {
                Ok(port) => {
                    backoff.reset();
                    self.state.store(&self.id, LifecycleState::Connected);
                    let mut client = Client::new(
                        port,
                        ClientConfig {
                            response_timeout: self.config.timeout,
                            wakeup_bytes: WAKEUP_BYTES,
                        },
                    );
                    if let Exit::Stop = self.run_connected(&mut client, &mut stop_rx).await {
                        self.state.store(&self.id, LifecycleState::Stopped);
                        self.set_comm_fault(true);
                        return;
                    }
                    self.set_comm_fault(true);
                }
                Err(err) => {
                    self.state.store(&self.id, LifecycleState::Failed);
                    warn!(
                        "[{}] 打开串口 {} 失败, 准备重试: {}",
                        self.id, self.config.serial_tty, err
                    );
                }
            }
            let delay = backoff.next_delay();
            tokio::select! {
                _ = time::sleep(delay) => {}
                _ = wait_stop(&mut stop_rx) => {
                    self.state.store(&self.id, LifecycleState::Stopped);
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
impl Dlt645Runner {
    /// 测试入口：在给定的 I/O 上运行轮询循环(跳过打开串口)。
    pub(super) async fn run_on<S>(self, io: S)
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut stop_rx = self.stop_rx.clone();
        let mut client = Client::new(
            io,
            ClientConfig {
                response_timeout: self.config.timeout,
                wakeup_bytes: WAKEUP_BYTES,
            },
        );
        self.set_comm_fault(true);
        let _ = self.run_connected(&mut client, &mut stop_rx).await;
        self.state.store(&self.id, LifecycleState::Stopped);
        self.set_comm_fault(true);
    }
}
