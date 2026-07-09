use std::collections::HashMap;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time;
use tokio_modbus::Slave;
use tokio_modbus::client::{Context, rtu, tcp};
use tokio_modbus::prelude::SlaveContext;
use tokio_serial::{DataBits, Parity};
use tracing::{info, warn};

use crate::center::SharedPointCenter;
use crate::config::modbus_conf::{ModbusConfig, ModbusConfigs};
use crate::core::point::{DataPoint, DownDataPoint, PointId, PointRef, Val};
use crate::dev::modbus_dev::Protocol;
use crate::dev::modbus_dev::block::{BlockRead, Blocks};
use crate::dev::modbus_dev::downlink::{WritePlan, build_cfg_map, build_key_map, build_name_map};
use crate::dev::{LifecycleState, state::SharedState};

use super::backoff::Backoff;
use super::error::ModbusDevError;

/// 连续读取失败（含超时）达到该阈值即判定连接不可用，触发重连
const MAX_READ_FAILURES: u32 = 3;

/// 三张点位查找表的打包引用，避免函数参数过多。
struct PointMaps<'a> {
    cfg_map: &'a HashMap<PointId, ModbusConfig>,
    key_map: &'a HashMap<&'static str, PointId>,
    name_map: &'a HashMap<&'static str, PointId>,
}

/// `drain_writes` 的结果
enum DrainOutcome {
    /// 写队列已排空；`true` 表示本轮确实下发过至少一次写入
    Idle(bool),
    /// 写入失败，需要断线重连
    WriteFailed,
    /// 上游 channel 已关闭，设备应停止运行
    ChannelClosed,
}

/// round-robin 读取一圈 block 的结果
enum ReadOutcome {
    /// 还未读满一圈，暂无可发布的数据
    Pending,
    /// 读满一圈，得到解析后的数据点（可能为空）
    Published(Vec<DataPoint>),
    /// 连续失败已达阈值，需要断线重连
    FailureThresholdReached,
}

/// round-robin 读取状态：当前游标、上一圈的槽位缓存、连续失败计数
struct ReadCursor {
    index: usize,
    block_count: usize,
    slots: Vec<Option<BlockRead>>,
    fail_streak: u32,
}

impl ReadCursor {
    fn new(block_count: usize) -> Self {
        Self {
            index: 0,
            block_count,
            slots: (0..block_count).map(|_| None).collect(),
            fail_streak: 0,
        }
    }

    /// 读取下一个 block，读满一圈后统一发布，语义与原周期读取一致
    async fn advance(
        &mut self,
        ctx: &mut Context,
        blocks: &Blocks,
        timeout: Duration,
        id: &str,
    ) -> ReadOutcome {
        if self.block_count == 0 {
            return ReadOutcome::Pending;
        }

        let i = self.index;
        self.index = (self.index + 1) % self.block_count;

        match time::timeout(timeout, blocks.request_one(ctx, i)).await {
            Ok(Ok(read)) => {
                self.fail_streak = 0;
                self.slots[i] = Some(read);
            }
            Ok(Err(err)) => {
                self.fail_streak += 1;
                warn!(
                    "[{}] 读取失败 ({}/{}): {}",
                    id, self.fail_streak, MAX_READ_FAILURES, err
                );
                if self.fail_streak >= MAX_READ_FAILURES {
                    return ReadOutcome::FailureThresholdReached;
                }
            }
            Err(_) => {
                self.fail_streak += 1;
                warn!(
                    "[{}] 读取超时 ({}/{}, 块 {})",
                    id, self.fail_streak, MAX_READ_FAILURES, i
                );
                if self.fail_streak >= MAX_READ_FAILURES {
                    return ReadOutcome::FailureThresholdReached;
                }
            }
        }

        if self.index != 0 {
            return ReadOutcome::Pending;
        }
        // 读完一圈：取出所有槽位数据，take() 同时将槽位复位为 None
        let reads: Vec<_> = self.slots.iter_mut().filter_map(|s| s.take()).collect();
        if reads.len() != self.block_count {
            return ReadOutcome::Pending;
        }
        ReadOutcome::Published(blocks.parse(&reads))
    }
}

pub(super) struct ModbusRunner {
    pub(super) id: String,
    pub(super) protocol: Protocol,
    pub(super) configs: ModbusConfigs,
    pub(super) state: SharedState,
    pub(super) stop_rx: watch::Receiver<bool>,
    pub(super) rx: mpsc::Receiver<Vec<DownDataPoint>>,
    pub(super) center: SharedPointCenter,
}

impl ModbusRunner {
    /// 上报通讯故障位：false = 有通讯，true = 无通讯。
    fn set_comm_fault(&self, fault: bool) {
        self.center.ingest(
            &self.id,
            vec![DataPoint {
                id: 0xFFFF,
                name: "通讯状态",
                value: Val::U8(fault as u8),
                key: "communicationStatus",
                translator: None,
                warn_bits: None,
                status_word: None,
                unit: None,
            }],
        );
    }

    fn stop_requested(stop_rx: &watch::Receiver<bool>) -> bool {
        *stop_rx.borrow()
    }

    fn timeout(&self) -> Duration {
        match &self.protocol {
            Protocol::Tcp(cfg) => Duration::from_millis(cfg.timeout),
            Protocol::Rtu(cfg) => Duration::from_millis(cfg.timeout),
        }
    }

    fn request_interval(&self) -> Duration {
        match &self.protocol {
            Protocol::Tcp(cfg) => Duration::from_millis(cfg.request_interval),
            Protocol::Rtu(cfg) => Duration::from_millis(cfg.request_interval),
        }
    }

    fn max_gap(&self) -> u16 {
        match &self.protocol {
            Protocol::Tcp(cfg) => cfg.max_gap,
            Protocol::Rtu(cfg) => cfg.max_gap,
        }
    }

    async fn connect(&self) -> Result<Context, ModbusDevError> {
        match &self.protocol {
            Protocol::Tcp(cfg) => {
                let addr = format!("{}:{}", cfg.ip, cfg.port).parse()?;
                let mut ctx = time::timeout(self.timeout(), tcp::connect(addr)).await??;
                ctx.set_slave(Slave(cfg.slave));
                Ok(ctx)
            }
            Protocol::Rtu(cfg) => {
                let mut builder = tokio_serial::new(cfg.serial_tty.as_str(), cfg.baudrate);
                builder = builder
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
                        2 => tokio_serial::StopBits::Two,
                        _ => tokio_serial::StopBits::One,
                    })
                    .timeout(self.timeout());
                let port = tokio_serial::SerialStream::open(&builder)?;
                let ctx = time::timeout(self.timeout(), async move {
                    Ok::<_, ModbusDevError>(rtu::attach_slave(port, Slave(cfg.slave)))
                })
                .await??;
                Ok(ctx)
            }
        }
    }

    /// 单请求调度器：每次节拍先排空写队列，再按 round-robin 读下一个块。
    ///
    /// 写延迟 ≤ request_interval，不随块数增长。
    /// 写入之后、以及每次读取之后都会等待一个 request_interval，
    /// 避免写完立刻读、或读请求过于密集导致从站/网关来不及响应。
    async fn run_connected(
        &mut self,
        ctx: &mut Context,
        stop_rx: &mut watch::Receiver<bool>,
        blocks: &Blocks,
        maps: PointMaps<'_>,
    ) {
        self.state.store(&self.id, LifecycleState::Running);
        let timeout = self.timeout();
        let effective_interval = self.request_interval().max(Duration::from_millis(1));

        let mut reader = ReadCursor::new(blocks.block_count());

        loop {
            if Self::stop_requested(stop_rx) {
                self.set_comm_fault(true);
                return;
            }

            match self.drain_writes(ctx, &maps).await {
                DrainOutcome::Idle(wrote_any) => {
                    // 写后至少让从站/网关喘一口气，避免写完立刻读导致超时
                    if wrote_any && Self::wait_interval(stop_rx, effective_interval).await {
                        self.set_comm_fault(true);
                        return;
                    }
                }
                DrainOutcome::WriteFailed => {
                    self.set_comm_fault(true);
                    return;
                }
                DrainOutcome::ChannelClosed => return,
            }

            match reader.advance(ctx, blocks, timeout, &self.id).await {
                ReadOutcome::Published(entries) => {
                    if !entries.is_empty() {
                        self.center.ingest(&self.id, entries);
                    }
                }
                ReadOutcome::Pending => {}
                ReadOutcome::FailureThresholdReached => {
                    self.set_comm_fault(true);
                    return;
                }
            }

            // 块间间隔：至少 1ms，防止 request_interval=0 时循环不挂起导致单核 100%
            if Self::wait_interval(stop_rx, effective_interval).await {
                self.set_comm_fault(true);
                return;
            }
        }
    }

    /// 等待 interval 或直到收到停止信号；返回 true 表示应停止
    async fn wait_interval(stop_rx: &mut watch::Receiver<bool>, interval: Duration) -> bool {
        tokio::select! {
            _ = time::sleep(interval) => false,
            _ = stop_rx.changed() => Self::stop_requested(stop_rx),
        }
    }

    /// 非阻塞排空写队列：处理所有已到达的写命令，队列空时立即返回。
    async fn drain_writes(&mut self, ctx: &mut Context, maps: &PointMaps<'_>) -> DrainOutcome {
        let mut wrote_any = false;
        loop {
            match self.rx.try_recv() {
                Ok(entries) => {
                    let items: Vec<String> = entries
                        .iter()
                        .map(|e| format!("{}: {}", resolve_name(&e.point, maps.cfg_map), e.value))
                        .collect();
                    info!("[{}] ↓: {}", self.id, items.join(", "));
                    let plan = WritePlan::build(
                        entries,
                        maps.cfg_map,
                        maps.key_map,
                        maps.name_map,
                        &self.id,
                    );
                    if let Err(err) = plan.apply(ctx).await {
                        warn!("[{}] 下发失败, 准备重连: {}", self.id, err);
                        return DrainOutcome::WriteFailed;
                    }
                    wrote_any = true;
                }
                Err(mpsc::error::TryRecvError::Empty) => return DrainOutcome::Idle(wrote_any),
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.set_comm_fault(true);
                    self.state.store(&self.id, LifecycleState::Stopped);
                    return DrainOutcome::ChannelClosed;
                }
            }
        }
    }

    pub(super) async fn run(mut self) {
        let cfg_map = build_cfg_map(&self.configs);
        let key_map = build_key_map(&self.configs);
        let name_map = build_name_map(&self.configs);
        let blocks = match Blocks::build(self.configs.clone(), self.max_gap()) {
            Ok(blocks) => blocks,
            Err(err) => {
                warn!("[{}] 构建读取块失败: {}", self.id, err);
                self.state.store(&self.id, LifecycleState::Failed);
                self.set_comm_fault(true);
                return;
            }
        };
        let mut stop_rx = self.stop_rx.clone();
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        loop {
            if Self::stop_requested(&stop_rx) {
                self.state.store(&self.id, LifecycleState::Stopped);
                self.set_comm_fault(true);
                return;
            }
            self.state.store(&self.id, LifecycleState::Connecting);
            self.set_comm_fault(true);

            match self.connect().await {
                Ok(mut ctx) => {
                    backoff.reset();
                    self.state.store(&self.id, LifecycleState::Connected);
                    self.set_comm_fault(false);
                    self.run_connected(
                        &mut ctx,
                        &mut stop_rx,
                        &blocks,
                        PointMaps {
                            cfg_map: &cfg_map,
                            key_map: &key_map,
                            name_map: &name_map,
                        },
                    )
                    .await;
                }
                Err(err) => {
                    self.state.store(&self.id, LifecycleState::Failed);
                    warn!("[{}] 连接失败, 准备重连: {}", self.id, err);
                    self.set_comm_fault(true);
                }
            }
            if Self::stop_requested(&stop_rx) {
                self.state.store(&self.id, LifecycleState::Stopped);
                self.set_comm_fault(true);
                return;
            }
            let delay = backoff.next_delay();
            tokio::select! {
                _ = time::sleep(delay) => {}
                _ = stop_rx.changed() => {
                    if Self::stop_requested(&stop_rx) {
                        self.state.store(&self.id, LifecycleState::Stopped);
                        return;
                    }
                }
            }
        }
    }
}

fn resolve_name<'a>(point: &'a PointRef, cfg_map: &'a HashMap<PointId, ModbusConfig>) -> &'a str {
    match point {
        PointRef::Key(k) | PointRef::Name(k) => cfg_map
            .values()
            .find(|cfg| cfg.key == k)
            .map(|cfg| cfg.name)
            .unwrap_or("unknown"),
        PointRef::Id(id) => cfg_map.get(id).map(|cfg| cfg.name).unwrap_or("unknown"),
    }
}
