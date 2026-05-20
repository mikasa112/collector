use std::{
    collections::HashMap,
    net::SocketAddr,
    path::Path,
    sync::Arc,
};

use parking_lot::RwLock;

use futures::future;
use tokio::sync::mpsc;
use tokio_modbus::server::tcp::{Server, accept_tcp_connection};
use tokio_modbus::{ExceptionCode, Response, SlaveRequest};

use crate::{
    center::SharedPointCenter,
    config::{
        modbus_conf::RegisterType,
        north_modbus_conf::{
            NorthboundConfig, NorthboundConfigs, NorthboundConfigsError, RegValue, build_configs,
        },
    },
    dock::modbus::tables::RegisterTable,
    down,
    shutdown::ShutdownManager,
};

enum WriteRequest {
    SingleRegister(u16, u16),
    SingleCoil(u16, bool),
    MultipleRegisters(u16, Vec<u16>),
    MultipleCoils(u16, Vec<bool>),
}

struct ServiceState {
    table: RwLock<Arc<RegisterTable>>,
    write_tx: mpsc::Sender<WriteRequest>,
}

/// Modbus 服务实例，实现 tokio_modbus::server::Service。
/// 内部通过 Arc 共享状态，Clone 代价极低，每个连接独立持有一份引用。
#[derive(Clone)]
pub struct ModbusService(Arc<ServiceState>);

impl tokio_modbus::server::Service for ModbusService {
    type Request = SlaveRequest<'static>;
    type Response = Response;
    type Exception = ExceptionCode;
    type Future = future::Ready<Result<Self::Response, Self::Exception>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        future::ready(self.handle(req.request))
    }
}

impl ModbusService {
    /// 读锁只持有 Arc clone 的瞬间，随后无锁访问表数据
    fn table(&self) -> Arc<RegisterTable> {
        self.0.table.read().clone()
    }

    fn handle(&self, req: tokio_modbus::Request<'static>) -> Result<Response, ExceptionCode> {
        match req {
            tokio_modbus::Request::ReadCoils(addr, cnt) => {
                Ok(Response::ReadCoils(self.table().read_coils(addr, cnt)))
            }
            tokio_modbus::Request::ReadDiscreteInputs(addr, cnt) => Ok(
                Response::ReadDiscreteInputs(self.table().read_discrete_inputs(addr, cnt)),
            ),
            tokio_modbus::Request::ReadInputRegisters(addr, cnt) => Ok(
                Response::ReadInputRegisters(self.table().read_input_registers(addr, cnt)),
            ),
            tokio_modbus::Request::ReadHoldingRegisters(addr, cnt) => Ok(
                Response::ReadHoldingRegisters(self.table().read_holding_registers(addr, cnt)),
            ),
            tokio_modbus::Request::WriteSingleCoil(addr, value) => {
                let _ = self
                    .0
                    .write_tx
                    .try_send(WriteRequest::SingleCoil(addr, value));
                Ok(Response::WriteSingleCoil(addr, value))
            }
            tokio_modbus::Request::WriteMultipleCoils(addr, coils) => {
                let cnt = coils.len() as u16;
                let _ = self
                    .0
                    .write_tx
                    .try_send(WriteRequest::MultipleCoils(addr, coils.into_owned()));
                Ok(Response::WriteMultipleCoils(addr, cnt))
            }
            tokio_modbus::Request::WriteSingleRegister(addr, value) => {
                let _ = self
                    .0
                    .write_tx
                    .try_send(WriteRequest::SingleRegister(addr, value));
                Ok(Response::WriteSingleRegister(addr, value))
            }
            tokio_modbus::Request::WriteMultipleRegisters(addr, regs) => {
                let cnt = regs.len() as u16;
                let _ = self
                    .0
                    .write_tx
                    .try_send(WriteRequest::MultipleRegisters(addr, regs.into_owned()));
                Ok(Response::WriteMultipleRegisters(addr, cnt))
            }
            _ => Err(ExceptionCode::IllegalFunction),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ModbusServerError {
    #[error("{0}")]
    OpenConfigsError(#[from] NorthboundConfigsError),
}

pub struct ModbusServer {
    configs: Arc<NorthboundConfigs>,
    center: SharedPointCenter,
    addr: SocketAddr,
}

impl ModbusServer {
    pub fn new<P: AsRef<Path>>(
        path: P,
        addr: SocketAddr,
        center: SharedPointCenter,
    ) -> Result<Self, ModbusServerError> {
        let configs = build_configs(path)?;
        Ok(Self {
            configs: Arc::new(configs),
            center,
            addr,
        })
    }

    pub async fn start(self, shutdown: ShutdownManager) {
        let (write_tx, write_rx) = mpsc::channel::<WriteRequest>(32);

        let state = Arc::new(ServiceState {
            table: RwLock::new(Arc::new(RegisterTable::new())),
            write_tx,
        });

        // 按设备分组，构建 point_id -> 配置下标 的索引
        let mut by_dev: HashMap<String, HashMap<u32, Vec<usize>>> = HashMap::new();
        for (i, cfg) in self.configs.iter().enumerate() {
            by_dev
                .entry(cfg.point_source.source.clone())
                .or_default()
                .entry(cfg.point_source.point_id)
                .or_default()
                .push(i);
        }

        // 构建写操作查找索引 (addr -> 配置下标)，只对可写寄存器类型建索引
        let mut coil_index: HashMap<u16, usize> = HashMap::new();
        let mut holding_index: HashMap<u16, usize> = HashMap::new();
        for (i, cfg) in self.configs.iter().enumerate() {
            match cfg.register_type {
                RegisterType::Coils => {
                    coil_index.insert(cfg.register_address, i);
                }
                RegisterType::HoldingRegisters => {
                    holding_index.insert(cfg.register_address, i);
                }
                _ => {}
            }
        }
        let coil_index = Arc::new(coil_index);
        let holding_index = Arc::new(holding_index);

        // 为每个设备启动订阅任务，监听 DataCenter 更新并写入寄存器表
        for (dev, point_index) in by_dev {
            let Some(mut rx) = self.center.subscribe(&dev) else {
                tracing::warn!("[北向Modbus] 订阅 {} 失败，设备尚未注册", dev);
                continue;
            };
            let state = state.clone();
            let configs = self.configs.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown.wait_for_shutdown() => break,
                        changed = rx.changed() => {
                            if changed.is_err() { break; }
                            let snapshot = rx.borrow_and_update().clone();
                            // 在锁外构建新表，写锁只持有 Arc 交换的瞬间
                            let current = state.table.read().clone();
                            let mut new_tbl = (*current).clone();
                            for point in snapshot.iter() {
                                let Some(cfg_indices) = point_index.get(&point.id) else { continue; };
                                for &ci in cfg_indices {
                                    let cfg = &configs[ci];
                                    if let Some(reg_val) = cfg.encode_val(&point.value) {
                                        match reg_val {
                                            RegValue::Bool(b) => new_tbl.write_bool(cfg.register_type, cfg.register_address, b),
                                            RegValue::Word(w) => new_tbl.write_u16(cfg.register_type, cfg.register_address, w),
                                            RegValue::DWord(dw) => new_tbl.write_u16_pair(cfg.register_type, cfg.register_address, dw),
                                        }
                                    }
                                }
                            }
                            *state.table.write() = Arc::new(new_tbl);
                        }
                    }
                }
            });
        }

        // 写操作派发任务：接收写请求，异步下发到 DataCenter
        {
            let center = self.center.clone();
            let configs = self.configs.clone();
            let shutdown = shutdown.clone();
            tokio::spawn(async move {
                let mut rx = write_rx;
                loop {
                    tokio::select! {
                        _ = shutdown.wait_for_shutdown() => break,
                        Some(req) = rx.recv() => {
                            dispatch_write(&center, &configs, &coil_index, &holding_index, req).await;
                        }
                    }
                }
            });
        }

        // 启动 TCP 监听
        let listener = match tokio::net::TcpListener::bind(self.addr).await {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("[北向Modbus] 绑定 {} 失败: {}", self.addr, e);
                return;
            }
        };
        tracing::info!("[北向Modbus] 监听 {}", self.addr);

        let service = ModbusService(state);
        let on_connected = |stream, socket_addr| {
            let svc = service.clone();
            async move { accept_tcp_connection(stream, socket_addr, move |_| Ok(Some(svc.clone()))) }
        };

        let token = shutdown.token();
        let abort = Box::pin(async move { token.cancelled().await });
        let server = Server::new(listener);
        if let Err(e) = server
            .serve_until(
                &on_connected,
                |e| tracing::error!("[北向Modbus] 连接错误: {}", e),
                abort,
            )
            .await
        {
            tracing::error!("[北向Modbus] 服务器错误: {}", e);
        }
    }
}

async fn dispatch_write(
    center: &SharedPointCenter,
    configs: &[NorthboundConfig],
    coil_index: &HashMap<u16, usize>,
    holding_index: &HashMap<u16, usize>,
    req: WriteRequest,
) {
    match req {
        WriteRequest::SingleRegister(addr, value) => {
            if let Some(&ci) = holding_index.get(&addr) {
                let cfg = &configs[ci];
                tracing::info!("[北向Modbus] ↓ {}", cfg.name);
                let _ = center
                    .dispatch(
                        &cfg.point_source.source,
                        vec![down!(id: cfg.point_source.point_id, cfg.restore_val(value))],
                    )
                    .await;
            }
        }
        WriteRequest::SingleCoil(addr, value) => {
            if let Some(&ci) = coil_index.get(&addr) {
                let cfg = &configs[ci];
                tracing::info!("[北向Modbus] ↓ {}", cfg.name);
                let _ = center
                    .dispatch(
                        &cfg.point_source.source,
                        vec![
                            down!(id: cfg.point_source.point_id, cfg.restore_val(u16::from(value))),
                        ],
                    )
                    .await;
            }
        }
        WriteRequest::MultipleRegisters(addr, values) => {
            for (offset, value) in values.into_iter().enumerate() {
                let a = addr.saturating_add(offset as u16);
                if let Some(&ci) = holding_index.get(&a) {
                    let cfg = &configs[ci];
                    tracing::info!("[北向Modbus] ↓ {}", cfg.name);
                    let _ = center
                        .dispatch(
                            &cfg.point_source.source,
                            vec![down!(id: cfg.point_source.point_id, cfg.restore_val(value))],
                        )
                        .await;
                }
            }
        }
        WriteRequest::MultipleCoils(addr, values) => {
            for (offset, value) in values.into_iter().enumerate() {
                let a = addr.saturating_add(offset as u16);
                if let Some(&ci) = coil_index.get(&a) {
                    let cfg = &configs[ci];
                    tracing::info!("[北向Modbus] ↓ {}", cfg.name);
                    let _ = center
                        .dispatch(&cfg.point_source.source, vec![down!(id: cfg.point_source.point_id, cfg.restore_val(u16::from(value)))])
                        .await;
                }
            }
        }
    }
}
