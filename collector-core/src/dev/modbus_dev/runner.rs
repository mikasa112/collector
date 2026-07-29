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
use crate::dev::modbus_dev::downlink::{
    WriteOutcome, WritePlan, build_cfg_map, build_key_map, build_name_map, stop_requested,
};
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
                bits: None,
                words: None,
                unit: None,
            }],
        );
    }

    fn timeout(&self) -> Duration {
        match &self.protocol {
            Protocol::Tcp(cfg) => Duration::from_millis(cfg.timeout),
            Protocol::Rtu(cfg) => Duration::from_millis(cfg.timeout),
        }
    }

    /// 单个block请求后间隔
    fn request_interval(&self) -> Duration {
        match &self.protocol {
            Protocol::Tcp(cfg) => Duration::from_millis(cfg.request_interval),
            Protocol::Rtu(cfg) => Duration::from_millis(cfg.request_interval),
        }
    }

    /// 每次大循环轮询间隔
    fn interval(&self) -> Duration {
        match &self.protocol {
            Protocol::Tcp(cfg) => Duration::from_millis(cfg.interval),
            Protocol::Rtu(cfg) => Duration::from_millis(cfg.interval),
        }
    }

    /// 写相关等待使用的固定间隔：`request_interval`，下限 1ms，不随块数增长。
    fn write_interval(&self) -> Duration {
        self.request_interval().max(Duration::from_millis(1))
    }

    /// 读取的块间等待间隔：`interval == 0` 时沿用 `write_interval`；否则将
    /// `interval` 按 `block_count` 均分（下限 1ms），使一整轮读取的总耗时
    /// 贴近配置值，而不是随 block 数线性放大。
    fn read_interval(&self, block_count: usize) -> Duration {
        let interval = self.interval();
        if interval.is_zero() {
            self.write_interval()
        } else {
            let block_count = block_count.max(1) as u32;
            (interval / block_count).max(Duration::from_millis(1))
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

    /// 单请求调度器：读、写共用同一条连接，通过一个 `tokio::select!` 循环仲裁，
    /// 每次循环只处理一个到位的事件（停止信号 / 一批下行写命令 / 一次读 tick）。
    ///
    /// 三路按 `biased` 顺序仲裁优先级：停止 > 写 > 读，与之前“先排空写队列、
    /// 再读一个 block”的语义一致——写命令一旦到达即被立即处理，不会被读节拍
    /// 挡住，写延迟始终 ≤ `write_interval`（即 `request_interval`，下限 1ms，
    /// 不随块数增长）。[`WritePlan::apply`] 在每次实际写入后（含最后一次）都会
    /// 等待一个 `write_interval`，天然让从站/网关喘一口气，因此这里无需再额外等待。
    ///
    /// 读节拍由 `ticker`（周期 `read_interval`）驱动：`interval == 0` 时
    /// `read_interval` 退化为 `write_interval`（下限 1ms，防止忙等占满单核）；
    /// `interval != 0` 时按 block 数均分，使一整轮读取的总耗时贴近配置值，而不是
    /// 随 block 数线性放大，从而降低整体 CPU 占用。`ticker` 使用
    /// `MissedTickBehavior::Delay`，避免某次写入耗时较长时后续读取“追帧”爆发。
    async fn run_connected(
        &mut self,
        ctx: &mut Context,
        stop_rx: &mut watch::Receiver<bool>,
        blocks: &Blocks,
        maps: PointMaps<'_>,
    ) {
        self.state.store(&self.id, LifecycleState::Running);
        let timeout = self.timeout();
        let write_interval = self.write_interval();
        let read_interval = self.read_interval(blocks.block_count());

        let mut reader = ReadCursor::new(blocks.block_count());
        let mut ticker = time::interval(read_interval);
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                biased;

                _ = stop_rx.changed() => {
                    if stop_requested(stop_rx) {
                        self.set_comm_fault(true);
                        return;
                    }
                }

                maybe = self.rx.recv() => {
                    match maybe {
                        Some(entries) => {
                            match self
                                .apply_write_batch(ctx, &maps, stop_rx, entries, write_interval)
                                .await
                            {
                                Ok(WriteOutcome::Completed) => {}
                                Ok(WriteOutcome::Stopped) => {
                                    self.set_comm_fault(true);
                                    return;
                                }
                                Err(err) => {
                                    warn!("[{}] 下发失败, 准备重连: {}", self.id, err);
                                    self.set_comm_fault(true);
                                    return;
                                }
                            }
                        }
                        None => {
                            self.set_comm_fault(true);
                            self.state.store(&self.id, LifecycleState::Stopped);
                            return;
                        }
                    }
                }

                _ = ticker.tick() => {
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
                }
            }
        }
    }

    /// 构建并下发一批下行写命令：记录日志、构建 [`WritePlan`] 并调用
    /// [`WritePlan::apply`]。
    async fn apply_write_batch(
        &self,
        ctx: &mut Context,
        maps: &PointMaps<'_>,
        stop_rx: &mut watch::Receiver<bool>,
        entries: Vec<DownDataPoint>,
        interval: Duration,
    ) -> Result<WriteOutcome, ModbusDevError> {
        let items: Vec<String> = entries
            .iter()
            .map(|e| format!("{}: {}", resolve_name(&e.point, maps.cfg_map), e.value))
            .collect();
        info!("[{}] ↓: {}", self.id, items.join(", "));
        let plan = WritePlan::build(entries, maps.cfg_map, maps.key_map, maps.name_map, &self.id);
        plan.apply(ctx, self.timeout(), stop_rx, interval).await
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
            if stop_requested(&stop_rx) {
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
            if stop_requested(&stop_rx) {
                self.state.store(&self.id, LifecycleState::Stopped);
                self.set_comm_fault(true);
                return;
            }
            let delay = backoff.next_delay();
            tokio::select! {
                _ = time::sleep(delay) => {}
                _ = stop_rx.changed() => {
                    if stop_requested(&stop_rx) {
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
