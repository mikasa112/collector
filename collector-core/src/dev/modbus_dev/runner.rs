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
use crate::dev::modbus_dev::block::Blocks;
use crate::dev::modbus_dev::downlink::{WritePlan, build_cfg_map, build_key_map, build_name_map};
use crate::dev::{LifecycleState, state::SharedState};

use super::backoff::Backoff;
use super::error::ModbusDevError;

/// 三张点位查找表的打包引用，避免函数参数过多。
struct PointMaps<'a> {
    cfg: &'a HashMap<PointId, ModbusConfig>,
    key: &'a HashMap<&'static str, PointId>,
    name: &'a HashMap<&'static str, PointId>,
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
    /// 反馈通讯连接状态
    fn report_comm_status(&self, v: u8) {
        self.center.ingest(
            &self.id,
            vec![DataPoint {
                id: 0xFFFF,
                name: "通讯状态",
                value: Val::U8(v),
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

    fn poll_interval(&self) -> Duration {
        match &self.protocol {
            Protocol::Tcp(cfg) => Duration::from_millis(cfg.interval),
            Protocol::Rtu(cfg) => Duration::from_millis(cfg.interval),
        }
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

    /// 写优先的顺序读写
    ///
    /// 每个轮询周期开始前先非阻塞排空下行队列，优先保证控制命令响应时效，
    /// 再依次读取各数据块（块间仍交织处理下行，维持原有行为）。
    async fn run_connected_rtu(
        &mut self,
        ctx: &mut Context,
        stop_rx: &mut watch::Receiver<bool>,
        blocks: &Blocks,
        maps: PointMaps<'_>,
    ) {
        self.state.store(&self.id, LifecycleState::Running);
        self.report_comm_status(1);
        let mut ticker = time::interval(self.poll_interval());
        ticker.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = stop_rx.changed() => {
                    if Self::stop_requested(stop_rx) {
                        self.report_comm_status(0);
                        return;
                    }
                }
                _ = ticker.tick() => {
                    // 写优先：先排空队列中所有待发命令
                    if let Err(reconnect) = self.flush_pending_writes(ctx, &maps).await {
                        if reconnect {
                            self.report_comm_status(0);
                        }
                        return;
                    }
                    // 读取数据块，块间继续交织处理后续到达的下行命令
                    if let Err(reconnect) = self.read_and_interleave_downlinks(ctx, blocks, &maps).await {
                        if reconnect {
                            self.report_comm_status(0);
                        }
                        return;
                    }
                }
            }
        }
    }

    /// RTU 写优先辅助：非阻塞排空下行队列中待发命令，以 poll_interval 为时间预算。
    ///
    /// 避免无限制地消耗 tick 臂时间（屏蔽 stop 信号 / 饿死读操作）：
    /// 超出时间预算后立即返回，剩余写命令留给块间 `drain_downlinks` 处理。
    async fn flush_pending_writes(
        &mut self,
        ctx: &mut Context,
        maps: &PointMaps<'_>,
    ) -> Result<(), bool> {
        let deadline = tokio::time::Instant::now() + self.poll_interval();
        loop {
            match self.rx.try_recv() {
                Ok(entries) => {
                    let items: Vec<String> = entries
                        .iter()
                        .map(|e| {
                            format!("{}: {}", resolve_modbus_name(&e.point, maps.cfg), e.value)
                        })
                        .collect();
                    info!("[{}] ↓ (优先): {}", self.id, items.join(", "));
                    let plan = WritePlan::build(entries, maps.cfg, maps.key, maps.name, &self.id);
                    if let Err(err) = plan.apply(ctx).await {
                        warn!("[{}] 下发失败, 准备重连: {}", self.id, err);
                        return Err(true);
                    }
                    if tokio::time::Instant::now() >= deadline {
                        return Ok(());
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.report_comm_status(0);
                    self.state.store(&self.id, LifecycleState::Stopped);
                    return Err(false);
                }
            }
        }
    }

    /// 读取所有数据块并在块间交织处理下行命令（RTU 共用）
    async fn read_and_interleave_downlinks(
        &mut self,
        ctx: &mut Context,
        blocks: &Blocks,
        maps: &PointMaps<'_>,
    ) -> Result<(), bool> {
        let request_interval = self.request_interval();
        let timeout = self.timeout();
        let mut reads = Vec::with_capacity(blocks.block_count());

        for i in 0..blocks.block_count() {
            match time::timeout(timeout, blocks.request_one(ctx, i)).await {
                Ok(Ok(read)) => reads.push(read),
                Ok(Err(err)) => {
                    warn!("[{}] 读取失败, 准备重连: {}", self.id, err);
                    return Err(true);
                }
                Err(_) => {
                    warn!(
                        "[{}] 读取超时, 块信息: {}, 准备重连",
                        self.id,
                        blocks.describe()
                    );
                    return Err(true);
                }
            }

            self.drain_downlinks(ctx, maps, request_interval).await?;
        }

        let entries = blocks.parse(&reads);
        if !entries.is_empty() {
            self.center.ingest(&self.id, entries);
        }
        Ok(())
    }

    async fn drain_downlinks(
        &mut self,
        ctx: &mut Context,
        maps: &PointMaps<'_>,
        interval: Duration,
    ) -> Result<(), bool> {
        let deadline = tokio::time::Instant::now() + interval;
        loop {
            let msg = if interval.is_zero() {
                match self.rx.try_recv() {
                    Ok(entries) => Some(entries),
                    Err(tokio::sync::mpsc::error::TryRecvError::Empty) => None,
                    Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                        self.report_comm_status(0);
                        self.state.store(&self.id, LifecycleState::Stopped);
                        return Err(false);
                    }
                }
            } else {
                tokio::select! {
                    msg = self.rx.recv() => {
                        match msg {
                            Some(entries) => Some(entries),
                            None => {
                                self.report_comm_status(0);
                                self.state.store(&self.id, LifecycleState::Stopped);
                                return Err(false);
                            }
                        }
                    }
                    _ = time::sleep_until(deadline) => None,
                }
            };

            let Some(entries) = msg else {
                return Ok(());
            };

            let items: Vec<String> = entries
                .iter()
                .map(|e| format!("{}: {}", resolve_modbus_name(&e.point, maps.cfg), e.value))
                .collect();
            info!("[{}] ↓: {}", self.id, items.join(", "));
            let plan = WritePlan::build(entries, maps.cfg, maps.key, maps.name, &self.id);
            if let Err(err) = plan.apply(ctx).await {
                warn!("[{}] 下发失败, 准备重连: {}", self.id, err);
                return Err(true);
            }

            if !interval.is_zero() && tokio::time::Instant::now() >= deadline {
                return Ok(());
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
                self.report_comm_status(0);
                return;
            }
        };
        let mut stop_rx = self.stop_rx.clone();
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        loop {
            if Self::stop_requested(&stop_rx) {
                self.state.store(&self.id, LifecycleState::Stopped);
                self.report_comm_status(0);
                return;
            }
            self.state.store(&self.id, LifecycleState::Connecting);
            self.report_comm_status(0);

            match self.connect().await {
                Ok(mut ctx) => {
                    backoff.reset();
                    self.state.store(&self.id, LifecycleState::Connected);
                    self.report_comm_status(1);
                    self.run_connected_rtu(
                        &mut ctx,
                        &mut stop_rx,
                        &blocks,
                        PointMaps {
                            cfg: &cfg_map,
                            key: &key_map,
                            name: &name_map,
                        },
                    )
                    .await;
                }
                Err(err) => {
                    self.state.store(&self.id, LifecycleState::Failed);
                    warn!("[{}] 连接失败, 准备重连: {}", self.id, err);
                    self.report_comm_status(0);
                }
            }
            if Self::stop_requested(&stop_rx) {
                self.state.store(&self.id, LifecycleState::Stopped);
                self.report_comm_status(0);
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

fn resolve_modbus_name<'a>(
    point: &'a PointRef,
    cfg_map: &'a HashMap<PointId, ModbusConfig>,
) -> &'a str {
    match point {
        PointRef::Key(k) | PointRef::Name(k) => k,
        PointRef::Id(id) => cfg_map.get(id).map(|cfg| cfg.name).unwrap_or("unknown"),
    }
}
