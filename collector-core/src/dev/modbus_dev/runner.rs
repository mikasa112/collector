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
    /// 读满一圈（block_count 个块）后统一发布，语义与原周期读取一致。
    async fn run_connected(
        &mut self,
        ctx: &mut Context,
        stop_rx: &mut watch::Receiver<bool>,
        blocks: &Blocks,
        maps: PointMaps<'_>,
    ) {
        self.state.store(&self.id, LifecycleState::Running);
        let block_count = blocks.block_count();
        let request_interval = self.request_interval();
        let timeout = self.timeout();

        let mut cursor = 0usize;
        // 每个槽位缓存最近一次读值，积满一圈后整体发布
        let mut read_slots: Vec<Option<_>> = (0..block_count).map(|_| None).collect();
        // 连续失败计数，达到阈值才触发重连
        let mut fail_streak = 0u32;
        const MAX_FAILURES: u32 = 3;

        loop {
            if Self::stop_requested(stop_rx) {
                self.set_comm_fault(true);
                return;
            }

            if let Err(fault) = self.drain_writes(ctx, &maps).await {
                if fault {
                    self.set_comm_fault(true);
                }
                return;
            }

            if block_count > 0 {
                let i = cursor;
                cursor = (cursor + 1) % block_count;

                match time::timeout(timeout, blocks.request_one(ctx, i)).await {
                    Ok(Ok(read)) => {
                        fail_streak = 0;
                        read_slots[i] = Some(read);
                    }
                    Ok(Err(err)) => {
                        fail_streak += 1;
                        warn!(
                            "[{}] 读取失败 ({}/{}): {}",
                            self.id, fail_streak, MAX_FAILURES, err
                        );
                        if fail_streak >= MAX_FAILURES {
                            self.set_comm_fault(true);
                            return;
                        }
                    }
                    Err(_) => {
                        fail_streak += 1;
                        warn!(
                            "[{}] 读取超时 ({}/{}, 块 {})",
                            self.id, fail_streak, MAX_FAILURES, i
                        );
                        if fail_streak >= MAX_FAILURES {
                            self.set_comm_fault(true);
                            return;
                        }
                    }
                }

                // 读完一圈 → 发布；take() 同时将槽位复位为 None
                if cursor == 0 {
                    let reads: Vec<_> = read_slots.iter_mut().filter_map(|s| s.take()).collect();
                    if reads.len() == block_count {
                        let entries = blocks.parse(&reads);
                        if !entries.is_empty() {
                            self.center.ingest(&self.id, entries);
                        }
                    }
                }
            }

            // 块间间隔：等待期间同步响应停止信号；yield 保证 interval=0 时不饥饿
            if !request_interval.is_zero() {
                tokio::select! {
                    _ = time::sleep(request_interval) => {}
                    _ = stop_rx.changed() => {
                        if Self::stop_requested(stop_rx) {
                            self.set_comm_fault(true);
                            return;
                        }
                    }
                }
            } else {
                tokio::task::yield_now().await;
            }
        }
    }

    /// 非阻塞排空写队列：处理所有已到达的写命令，队列空时立即返回。
    ///
    /// 返回 `Err(true)` 表示写失败需重连，`Err(false)` 表示 channel 已关闭。
    async fn drain_writes(&mut self, ctx: &mut Context, maps: &PointMaps<'_>) -> Result<(), bool> {
        loop {
            match self.rx.try_recv() {
                Ok(entries) => {
                    let items: Vec<String> = entries
                        .iter()
                        .map(|e| format!("{}: {}", resolve_name(&e.point, maps.cfg), e.value))
                        .collect();
                    info!("[{}] ↓: {}", self.id, items.join(", "));
                    let plan = WritePlan::build(entries, maps.cfg, maps.key, maps.name, &self.id);
                    if let Err(err) = plan.apply(ctx).await {
                        warn!("[{}] 下发失败, 准备重连: {}", self.id, err);
                        return Err(true);
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => return Ok(()),
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.set_comm_fault(true);
                    self.state.store(&self.id, LifecycleState::Stopped);
                    return Err(false);
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
