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
use crate::core::point::{DataPoint, DataPoints, PointId, Val};
use crate::dev::modbus_dev::Protocol;
use crate::dev::modbus_dev::block::Blocks;
use crate::dev::modbus_dev::downlink::{WritePlan, build_cfg_map};
use crate::dev::{LifecycleState, state::SharedState};

use super::backoff::Backoff;
use super::error::ModbusDevError;

pub(super) struct ModbusRunner {
    pub(super) id: String,
    pub(super) protocol: Protocol,
    pub(super) configs: ModbusConfigs,
    pub(super) state: SharedState,
    pub(super) stop_rx: watch::Receiver<bool>,
    pub(super) rx: mpsc::Receiver<Vec<DataPoint>>,
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
                name: "communication_status",
                value: Val::U8(v),
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
    ) {
        self.state.store(&self.id, LifecycleState::Running);
        self.report_comm_status(1);
        //读取任务的定时器
        let mut ticker = time::interval(self.poll_interval());
        loop {
            let rx = &mut self.rx;
            tokio::select! {
                _ = stop_rx.changed() => {
                    if Self::stop_requested(stop_rx) {
                        self.report_comm_status(0);
                        return;
                    }
                }
                //读取
                _ = ticker.tick() => {
                    match self.read_all(ctx, blocks).await {
                        Ok(entries) => {
                            if !entries.is_empty() {
                                //上送数据
                                self.center.ingest(&self.id, entries);
                            }
                        }
                        Err(err) => {
                            warn!("[{}] 读取失败, 准备重连: {}", self.id, err);
                            self.report_comm_status(0);
                            return;
                        }
                    }
                }
                //下发
                msg = rx.recv() => {
                    let Some(mut entries) = msg else {
                        self.report_comm_status(0);
                        self.state.store(&self.id, LifecycleState::Stopped);
                        return;
                    };

                    for entry in &mut entries {
                        if entry.name.is_empty() && let Some(cfg) = cfg_map.get(&entry.id) {
                            entry.name = cfg.name;
                        }
                    }

                    let points  = DataPoints(entries);
                    info!("[{}] ↓: {}", self.id, points);
                    let plan = WritePlan::build(points.0, cfg_map, &self.id);
                    if let Err(err) = plan.apply(ctx).await {
                        warn!("[{}] 下发失败, 准备重连: {}", self.id, err);
                        self.report_comm_status(0);
                        return;
                    }
                }
            }
        }
    }

    /// 启动MODBUS设备
    pub(super) async fn run(mut self) {
        let cfg_map = build_cfg_map(&self.configs);
        //构建连续地址的寄存器块
        let blocks = match Blocks::try_from(self.configs.clone()) {
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
                    self.run_connected(&mut ctx, &mut stop_rx, &blocks, &cfg_map)
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

    async fn read_all(
        &self,
        ctx: &mut Context,
        blocks: &Blocks,
    ) -> Result<Vec<DataPoint>, ModbusDevError> {
        let reads = blocks.request(ctx).await?;
        let parsed = blocks.parse(&reads);
        Ok(parsed)
    }
}
