use std::net::AddrParseError;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use crate::center::data_center::Entry;
use crate::center::{Center, DataCenterError, global_center};
use crate::config::modbus_conf::ModbusConfigs;
use crate::config::{self, Device};
use tokio::sync::{Mutex, mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_modbus::Slave;
use tokio_modbus::client::{Context, rtu, tcp};
use tokio_modbus::slave::SlaveContext;
use tokio_serial::{DataBits, Parity};
use tracing::{info, warn};

use crate::dev::{
    DeviceError, Executable, Identifiable, Lifecycle, LifecycleState,
    dev_config::{ModbusRtuConfig, ModbusTcpConfig},
};

#[derive(Debug, thiserror::Error)]
pub enum ModbusDevError {
    #[error("IP parse error: {0}")]
    IpParseError(#[from] AddrParseError),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Timeout")]
    Elapsed(#[from] tokio::time::error::Elapsed),
    #[error("Serial port error: {0}")]
    SerialError(#[from] tokio_serial::Error),
}

#[derive(Clone)]
pub enum Protocol {
    TCP(ModbusTcpConfig),
    RTU(ModbusRtuConfig),
}

pub struct ModbusDev {
    id: String,
    protocol: Protocol,
    configs: ModbusConfigs,
    state: Arc<AtomicU8>,
    tx: mpsc::Sender<Vec<Entry>>,
    rx: mpsc::Receiver<Vec<Entry>>,
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl ModbusDev {
    pub fn new(dev: Device) -> Result<Self, DeviceError> {
        let Some(id) = dev.id else {
            return Err(DeviceError::InvalidId);
        };
        let Some(com_type) = dev.config.com_type else {
            return Err(DeviceError::InvalidComType);
        };
        let Some(configs) = dev.protocol_configs else {
            return Err(DeviceError::NotFoundConfigs(id));
        };
        let configs = match configs {
            config::ProtocolConfigs::Modbus(modbus_configs) => modbus_configs,
            config::ProtocolConfigs::None => {
                return Err(DeviceError::NotFoundConfigs(id));
            }
        };
        let protocol = match com_type {
            config::ComType::ModbusTCP => {
                let tcp_config = ModbusTcpConfig::try_from(dev.config)?;
                Ok(Protocol::TCP(tcp_config))
            }
            config::ComType::ModbusRTU => {
                let rtu_config = ModbusRtuConfig::try_from(dev.config)?;
                Ok(Protocol::RTU(rtu_config))
            }
            _ => Err(DeviceError::UnSupportedComType),
        }?;
        let state = Arc::new(AtomicU8::new(LifecycleState::New as u8));
        let (tx, rx) = tokio::sync::mpsc::channel::<Vec<Entry>>(16);
        let (stop_tx, stop_rx) = watch::channel(false);
        info!("加载{}配置成功!", id);
        Ok(ModbusDev {
            id,
            protocol,
            state,
            configs,
            tx,
            rx,
            stop_tx,
            stop_rx,
            task: Mutex::new(None),
        })
    }

    fn load_state(&self) -> LifecycleState {
        load_state(&self.state)
    }

    fn cas_state(&self, from: LifecycleState, to: LifecycleState) -> bool {
        cas_state(&self.state, from, to)
    }

    fn store_state(&self, to: LifecycleState) {
        store_state(&self.id, &self.state, to);
    }
}

impl Identifiable for ModbusDev {
    fn id(&self) -> String {
        return self.id.clone();
    }
}

#[async_trait::async_trait]
impl Lifecycle for ModbusDev {
    fn init(&self) -> Result<(), DeviceError> {
        if !self.cas_state(LifecycleState::New, LifecycleState::Initializing) {
            return Ok(());
        }
        let tx = self.tx.clone();
        global_center().attach(self, tx)?;
        self.store_state(LifecycleState::Ready);
        Ok(())
    }

    async fn start(&mut self) -> Result<(), DeviceError> {
        let ok = self.cas_state(LifecycleState::Ready, LifecycleState::Starting)
            || self.cas_state(LifecycleState::Stopped, LifecycleState::Starting);
        if !ok {
            return Ok(());
        }
        let tx = self.tx.clone();
        match global_center().attach(self, tx) {
            Ok(()) => {}
            Err(DataCenterError::DevHasRegister(_)) => {}
            Err(err) => {
                warn!("[{}] 重新注册数据中心失败: {}", self.id, err);
            }
        }
        let _ = self.stop_tx.send(false);
        let mut task_guard = self.task.lock().await;
        if let Some(handle) = task_guard.take() {
            handle.abort();
        }
        let runner = ModbusRunner {
            id: self.id.clone(),
            protocol: self.protocol.clone(),
            state: Arc::clone(&self.state),
            stop_rx: self.stop_rx.clone(),
        };
        let handle = tokio::spawn(async move {
            runner.run().await;
        });
        *task_guard = Some(handle);
        Ok(())
    }

    async fn stop(&self) -> Result<(), DeviceError> {
        let _ = self.stop_tx.send(true);
        let cur = self.load_state();
        match cur {
            LifecycleState::Stopped => return Ok(()),
            LifecycleState::New | LifecycleState::Ready => {
                self.store_state(LifecycleState::Stopped);
                global_center().detach(self);
                return Ok(());
            }
            LifecycleState::Stopping => {}
            _ => {
                let _ = self.cas_state(cur, LifecycleState::Stopping);
            }
        }

        global_center().detach(self);
        let mut task_guard = self.task.lock().await;
        if let Some(mut handle) = task_guard.take() {
            tokio::select! {
                _ = time::sleep(Duration::from_secs(3)) => {
                    handle.abort();
                }
                _ = &mut handle => {}
            }
        }
        Ok(())
    }

    fn state(&self) -> LifecycleState {
        self.load_state()
    }
}

impl Executable for ModbusDev {}

struct ModbusRunner {
    id: String,
    protocol: Protocol,
    state: Arc<AtomicU8>,
    stop_rx: watch::Receiver<bool>,
}

impl ModbusRunner {
    fn stop_requested(stop_rx: &watch::Receiver<bool>) -> bool {
        *stop_rx.borrow()
    }

    fn poll_interval(&self) -> Duration {
        match &self.protocol {
            Protocol::TCP(cfg) => Duration::from_millis(cfg.interval),
            Protocol::RTU(cfg) => Duration::from_millis(cfg.interval),
        }
    }

    fn timeout(&self) -> Duration {
        match &self.protocol {
            Protocol::TCP(cfg) => Duration::from_millis(cfg.timeout),
            Protocol::RTU(cfg) => Duration::from_millis(cfg.timeout),
        }
    }

    async fn connect(&self) -> Result<Context, ModbusDevError> {
        match &self.protocol {
            Protocol::TCP(cfg) => {
                let addr = format!("{}:{}", cfg.ip, cfg.port).parse()?;
                let mut ctx = time::timeout(self.timeout(), tcp::connect(addr)).await??;
                ctx.set_slave(Slave(cfg.slave));
                Ok(ctx)
            }
            Protocol::RTU(cfg) => {
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

    async fn run_connected(&self, ctx: &mut Context, stop_rx: &mut watch::Receiver<bool>) {
        store_state(&self.id, &self.state, LifecycleState::Running);
        let mut ticker = time::interval(self.poll_interval());
        loop {
            tokio::select! {
                _ = stop_rx.changed() => {
                    if Self::stop_requested(stop_rx) {
                        return;
                    }
                }
                _ = ticker.tick() => {
                    // TODO: 读点位 + 上送
                    let _ = ctx;
                }
            }
        }
    }

    async fn run(&self) {
        let mut stop_rx = self.stop_rx.clone();
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        loop {
            if Self::stop_requested(&stop_rx) {
                store_state(&self.id, &self.state, LifecycleState::Stopped);
                return;
            }
            store_state(&self.id, &self.state, LifecycleState::Connecting);
            match self.connect().await {
                Ok(mut ctx) => {
                    store_state(&self.id, &self.state, LifecycleState::Connected);
                    backoff.reset();
                    self.run_connected(&mut ctx, &mut stop_rx).await;
                }
                Err(err) => {
                    store_state(&self.id, &self.state, LifecycleState::Failed);
                    warn!("[{}] 连接失败, 准备重连: {}", self.id, err);
                }
            }
            if Self::stop_requested(&stop_rx) {
                store_state(&self.id, &self.state, LifecycleState::Stopped);
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
}

fn load_state(state: &AtomicU8) -> LifecycleState {
    match state.load(Ordering::Acquire) {
        0 => LifecycleState::New,
        1 => LifecycleState::Initializing,
        2 => LifecycleState::Ready,
        3 => LifecycleState::Starting,
        4 => LifecycleState::Connecting,
        5 => LifecycleState::Connected,
        6 => LifecycleState::Running,
        7 => LifecycleState::Stopping,
        8 => LifecycleState::Stopped,
        9 => LifecycleState::Failed,
        _ => LifecycleState::Failed,
    }
}

fn cas_state(state: &AtomicU8, from: LifecycleState, to: LifecycleState) -> bool {
    state
        .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

fn store_state(id: &str, state: &AtomicU8, to: LifecycleState) {
    let from = load_state(state);
    state.store(to as u8, Ordering::Release);
    info!("[{}]{} -> {}", id, from, to);
}

struct Backoff {
    current: Duration,
    base: Duration,
    max: Duration,
}

impl Backoff {
    fn new(base: Duration, max: Duration) -> Self {
        Self {
            current: base,
            base,
            max,
        }
    }
    fn reset(&mut self) {
        self.current = self.base;
    }

    fn next_delay(&mut self) -> Duration {
        let delay = self.current;
        self.current = (self.current * 2).min(self.max);
        delay
    }
}
