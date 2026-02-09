use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::time::Duration;

use tokio::sync::{mpsc, watch};
use tokio::time;
use tokio_modbus::Slave;
use tokio_modbus::client::{Context, rtu, tcp};
use tokio_modbus::prelude::SlaveContext;
use tokio_serial::{DataBits, Parity};
use tracing::warn;

use crate::center::data_center::Entry;
use crate::center::{Center, global_center};
use crate::config::modbus_conf::{ModbusConfig, ModbusConfigs};
use crate::core::point::Val;
use crate::dev::modbus_dev::Protocol;
use crate::dev::modbus_dev::block::Blocks;
use crate::dev::modbus_dev::downlink::{WritePlan, apply_write_plan, build_cfg_map};
use crate::dev::{Identifiable, LifecycleState};

use super::backoff::Backoff;
use super::error::ModbusDevError;
use super::state::store_state;

pub(super) struct ModbusRunner {
    pub(super) id: String,
    pub(super) protocol: Protocol,
    pub(super) configs: ModbusConfigs,
    pub(super) state: Arc<AtomicU8>,
    pub(super) stop_rx: watch::Receiver<bool>,
    pub(super) rx: mpsc::Receiver<Vec<Entry>>,
}

impl Identifiable for ModbusRunner {
    fn id(&self) -> String {
        self.id.clone()
    }
}

impl ModbusRunner {
    fn report_comm_status(&self, v: u8) {
        global_center().ingest(
            self,
            vec![Entry {
                key: "COMM_STATUS".to_string(),
                value: Val::U8(v),
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

    async fn run_connected(
        &mut self,
        ctx: &mut Context,
        stop_rx: &mut watch::Receiver<bool>,
        cfg_map: &HashMap<String, ModbusConfig>,
    ) {
        store_state(&self.id, &self.state, LifecycleState::Running);
        self.report_comm_status(1);
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
                _ = ticker.tick() => {
                    match self.read_all(ctx).await {
                        Ok(entries) => {
                            if !entries.is_empty() {
                                global_center().ingest(self, entries);
                            }
                        }
                        Err(err) => {
                            warn!("[{}] 读取失败, 准备重连: {}", self.id, err);
                            self.report_comm_status(0);
                            return;
                        }
                    }
                }
                msg = rx.recv() => {
                    let Some(entries) = msg else {
                        self.report_comm_status(0);
                        store_state(&self.id, &self.state, LifecycleState::Stopped);
                        return;
                    };
                    let plan = WritePlan::build(entries, cfg_map, &self.id);
                    if let Err(err) = apply_write_plan(ctx, plan).await {
                        warn!("[{}] 下发失败, 准备重连: {}", self.id, err);
                        self.report_comm_status(0);
                        return;
                    }
                }
            }
        }
    }

    pub(super) async fn run(mut self) {
        let cfg_map = build_cfg_map(&self.configs);
        let mut stop_rx = self.stop_rx.clone();
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        loop {
            if Self::stop_requested(&stop_rx) {
                store_state(&self.id, &self.state, LifecycleState::Stopped);
                self.report_comm_status(0);
                return;
            }
            store_state(&self.id, &self.state, LifecycleState::Connecting);
            self.report_comm_status(0);
            match self.connect().await {
                Ok(mut ctx) => {
                    store_state(&self.id, &self.state, LifecycleState::Connected);
                    self.report_comm_status(1);
                    backoff.reset();
                    self.run_connected(&mut ctx, &mut stop_rx, &cfg_map).await;
                }
                Err(err) => {
                    store_state(&self.id, &self.state, LifecycleState::Failed);
                    warn!("[{}] 连接失败, 准备重连: {}", self.id, err);
                    self.report_comm_status(0);
                }
            }
            if Self::stop_requested(&stop_rx) {
                store_state(&self.id, &self.state, LifecycleState::Stopped);
                self.report_comm_status(0);
                return;
            }
            let delay = backoff.next_delay();
            tokio::select! {
                _ = time::sleep(delay) => {}
                _ = stop_rx.changed() => {
                    if Self::stop_requested(&stop_rx) {
                        store_state(&self.id, &self.state, LifecycleState::Stopped);
                        return;
                    }
                }
            }
        }
    }

    async fn read_all(&self, ctx: &mut Context) -> Result<Vec<Entry>, ModbusDevError> {
        let configs = self.configs.iter().collect::<Vec<&ModbusConfig>>();
        let mut blocks = Blocks::try_from(configs)?;
        let reads = blocks.request(ctx).await?;
        let parsed = blocks.parse(&reads);
        Ok(parsed
            .into_iter()
            .map(|(key, value)| Entry { key, value })
            .collect())
    }
}
