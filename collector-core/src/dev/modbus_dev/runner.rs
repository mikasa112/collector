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
use crate::dev::modbus_dev::downlink::{WritePlan, build_cfg_map, build_key_map};
use crate::dev::{LifecycleState, state::SharedState};

use super::backoff::Backoff;
use super::error::ModbusDevError;

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
    /// # 输入
    /// - `v`: 状态值，0表示正常，非0表示异常
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
            }],
        );
    }

    fn stop_requested(stop_rx: &watch::Receiver<bool>) -> bool {
        *stop_rx.borrow()
    }

    /// 获取轮询间隔时间
    /// # 输出
    /// - `Duration`: 轮询间隔时间
    fn poll_interval(&self) -> Duration {
        match &self.protocol {
            Protocol::Tcp(cfg) => Duration::from_millis(cfg.interval),
            Protocol::Rtu(cfg) => Duration::from_millis(cfg.interval),
        }
    }

    /// 获取超时时间
    /// # 输出
    /// - `Duration`: 超时时间
    fn timeout(&self) -> Duration {
        match &self.protocol {
            Protocol::Tcp(cfg) => Duration::from_millis(cfg.timeout),
            Protocol::Rtu(cfg) => Duration::from_millis(cfg.timeout),
        }
    }

    /// 获取每次 block 请求之间的间隔时间
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

    /// 获取MODBUS的连接
    /// # 输入
    /// - `self`: 当前的Modbus设备实例
    /// # 输出
    /// - `Result<Context, ModbusDevError>`: 连接结果，成功返回Context，失败返回ModbusDevError
    async fn connect(&self) -> Result<Context, ModbusDevError> {
        //配置连接的协议是TCP还是串口
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

    /// 连接成功后运行
    /// # 输入
    /// - `self`: 当前的Modbus设备实例
    /// - `ctx`: 连接的上下文
    /// - `stop_rx`: 停止信号接收器
    /// - `blocks`: 块信息
    /// - `cfg_map`: 配置映射
    async fn run_connected(
        &mut self,
        ctx: &mut Context,
        stop_rx: &mut watch::Receiver<bool>,
        blocks: &Blocks,
        cfg_map: &HashMap<PointId, ModbusConfig>,
        key_map: &HashMap<&'static str, PointId>,
    ) {
        self.state.store(&self.id, LifecycleState::Running);
        self.report_comm_status(1);
        let mut ticker = time::interval(self.poll_interval());
        loop {
            tokio::select! {
                _ = stop_rx.changed() => {
                    if Self::stop_requested(stop_rx) {
                        self.report_comm_status(0);
                        return;
                    }
                }
                _ = ticker.tick() => {
                    if let Err(reconnect) = self.read_and_interleave_downlinks(ctx, blocks, cfg_map, key_map).await {
                        if reconnect {
                            self.report_comm_status(0);
                        }
                        return;
                    }
                }
            }
        }
    }

    /// 逐 block 读取，每个 block 完成后排空下发队列，实现下发低延迟插入
    /// 返回 Err(true) 表示需要重连，Err(false) 表示 channel 关闭需停止
    async fn read_and_interleave_downlinks(
        &mut self,
        ctx: &mut Context,
        blocks: &Blocks,
        cfg_map: &HashMap<PointId, ModbusConfig>,
        key_map: &HashMap<&'static str, PointId>,
    ) -> Result<(), bool> {
        let request_interval = self.request_interval();
        let timeout = self.timeout();
        let mut reads = Vec::with_capacity(blocks.block_count());

        for i in 0..blocks.block_count() {
            // 读取单个 block（带超时）
            match time::timeout(timeout, blocks.request_one(ctx, i)).await {
                Ok(Ok(read)) => reads.push(read),
                Ok(Err(err)) => {
                    warn!("[{}] 读取失败, 准备重连: {}", self.id, err);
                    return Err(true);
                }
                Err(_) => {
                    warn!("[{}] 读取超时, 块信息: {}, 准备重连", self.id, blocks.describe());
                    return Err(true);
                }
            }

            // block 间隙：排空下发队列，同时等待 request_interval
            if let Err(reconnect) = self.drain_downlinks(ctx, cfg_map, key_map, request_interval).await {
                return Err(reconnect);
            }
        }

        let entries = blocks.parse(&reads);
        if !entries.is_empty() {
            self.center.ingest(&self.id, entries);
        }
        Ok(())
    }

    /// 在 request_interval 时间窗口内，尽量排空下发 channel
    /// 返回 Err(true) 表示下发失败需重连，Err(false) 表示 channel 关闭需停止
    async fn drain_downlinks(
        &mut self,
        ctx: &mut Context,
        cfg_map: &HashMap<PointId, ModbusConfig>,
        key_map: &HashMap<&'static str, PointId>,
        interval: Duration,
    ) -> Result<(), bool> {
        let deadline = tokio::time::Instant::now() + interval;
        loop {
            // try_recv 先排空已有的，没有则等到 interval 结束
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

            let items: Vec<String> = entries.iter().map(|e| format!("{}: {}", resolve_modbus_name(&e.point, cfg_map), e.value)).collect();
            info!("[{}] ↓: {}", self.id, items.join(", "));
            let plan = WritePlan::build(entries, cfg_map, key_map, &self.id);
            if let Err(err) = plan.apply(ctx).await {
                warn!("[{}] 下发失败, 准备重连: {}", self.id, err);
                return Err(true);
            }

            // 下发完继续循环，用 try_recv 排空剩余积压
            // 若 interval 已到就直接退出
            if !interval.is_zero() && tokio::time::Instant::now() >= deadline {
                return Ok(());
            }
        }
    }

    /// 启动MODBUS设备
    pub(super) async fn run(mut self) {
        let cfg_map = build_cfg_map(&self.configs);
        let key_map = build_key_map(&self.configs);
        //构建连续地址的寄存器块
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
        //退避重连
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
                    self.state.store(&self.id, LifecycleState::Connected);
                    self.report_comm_status(1);
                    backoff.reset();
                    self.run_connected(&mut ctx, &mut stop_rx, &blocks, &cfg_map, &key_map)
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
