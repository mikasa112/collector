use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, RwLock},
};

use futures::future;
use tokio_modbus::{ExceptionCode, Response, SlaveRequest};

use crate::{
    center::SharedPointCenter,
    config::{
        modbus_conf::RegisterType,
        north_modbus_conf::{
            NorthboundConfig, NorthboundConfigs, NorthboundConfigsError, build_configs,
        },
    },
    dock::modbus::tables::RegisterTable,
    down,
    shutdown::ShutdownManager,
};

pub struct ModbusServer {
    configs: NorthboundConfigs,
    table: Arc<RwLock<RegisterTable>>,
    center: SharedPointCenter,
}

impl tokio_modbus::server::Service for ModbusServer {
    type Request = SlaveRequest<'static>;

    type Response = Response;

    type Exception = ExceptionCode;

    type Future = future::Ready<Result<Self::Response, Self::Exception>>;

    fn call(&self, req: Self::Request) -> Self::Future {
        let result = match req.request {
            tokio_modbus::Request::ReadCoils(addr, cnt) => match self.table.try_read() {
                Ok(tbl) => Ok(Response::ReadCoils(tbl.read_coils(addr, cnt))),
                Err(_) => Err(ExceptionCode::ServerDeviceBusy),
            },
            tokio_modbus::Request::ReadDiscreteInputs(addr, cnt) => match self.table.try_read() {
                Ok(tbl) => Ok(Response::ReadDiscreteInputs(
                    tbl.read_discrete_inputs(addr, cnt),
                )),
                Err(_) => Err(ExceptionCode::ServerDeviceBusy),
            },
            tokio_modbus::Request::WriteSingleCoil(_addr, _) => todo!(),
            tokio_modbus::Request::WriteMultipleCoils(_addr, _cow) => todo!(),
            tokio_modbus::Request::ReadInputRegisters(addr, cnt) => match self.table.try_read() {
                Ok(tbl) => Ok(Response::ReadInputRegisters(
                    tbl.read_input_registers(addr, cnt),
                )),
                Err(_) => Err(ExceptionCode::ServerDeviceBusy),
            },
            tokio_modbus::Request::ReadHoldingRegisters(addr, cnt) => match self.table.try_read() {
                Ok(tbl) => Ok(Response::ReadHoldingRegisters(
                    tbl.read_holding_registers(addr, cnt),
                )),
                Err(_) => Err(ExceptionCode::ServerDeviceBusy),
            },
            tokio_modbus::Request::WriteSingleRegister(_addr, _) => {
                todo!()
            }
            tokio_modbus::Request::WriteMultipleRegisters(_addr, _cow) => todo!(),
            _ => Err(ExceptionCode::IllegalFunction),
        };
        future::ready(result)
    }
}

#[derive(thiserror::Error, Debug)]
pub enum ModbusServerError {
    #[error("{0}")]
    OpenConfigsError(#[from] NorthboundConfigsError),
}

impl ModbusServer {
    pub fn new<P: AsRef<Path>>(
        path: P,
        data_center: SharedPointCenter,
    ) -> Result<Self, ModbusServerError> {
        let configs = build_configs(path)?;
        Ok(Self {
            configs,
            table: Arc::new(RwLock::new(RegisterTable::new())),
            center: data_center,
        })
    }

    pub async fn start(self, shutdown: ShutdownManager) {
        let mut by_dev: HashMap<&str, Vec<&NorthboundConfig>> = HashMap::new();
        for cfg in self.configs.iter() {
            by_dev
                .entry(&cfg.point_source.source)
                .or_default()
                .push(cfg);
        }
        for (dev, ads_vec) in by_dev {
            let Some(mut rx) = self.center.subscribe(dev) else {
                tracing::warn!("[北向Modbus] 订阅 {} 失败，设备尚未注册", dev);
                continue;
            };
            let shutdown = shutdown.clone();
            let table = self.table.clone();

            {
                // 1. 从datacenter中找到对应的点
                // 2. 将点映射到table中
                // let snapshot = rx.borrow_and_update().clone();
                // let table_w = match table.write() {
                //     Ok(write) => write,
                //     Err(e) => {
                //         tracing::warn!("[北向Modbus] 写锁错误: {e}");
                //         continue;
                //     }
                // };
                // for point in snapshot.iter() {
                //     if let Some(c) = ads_vec.iter().find(|it| {
                //         it.point_source.point_key == point.key
                //             && it.point_source.point_id == point.id
                //     }) {
                //         table_w.write_u16(c.register_type, c.register_address, point.value);
                //     }
                // }
            }
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        _ = shutdown.wait_for_shutdown() => {
                            break;
                        }
                        _ = rx.changed() => {

                        }
                    }
                }
            });
        }
    }

    async fn write_single_register(&self, addr: u16, value: u16) {
        if let Some(config) = self.configs.iter().find(|it| {
            it.register_address == addr && it.register_type == RegisterType::HoldingRegisters
        }) {
            let _ = self
                .center
                .dispatch(
                    config.point_source.source.as_str(),
                    vec![down!(id: config.point_source.point_id, config.restore_val(value))],
                )
                .await;
        }
    }

    async fn write_single_coil(&self, addr: u16, value: bool) {
        let value = if value { 1 } else { 0 };
        if let Some(config) = self
            .configs
            .iter()
            .find(|it| it.register_address == addr && it.register_type == RegisterType::Coils)
        {
            let _ = self
                .center
                .dispatch(
                    config.point_source.source.as_str(),
                    vec![down!(id: config.point_source.point_id, config.restore_val(value))],
                )
                .await;
        }
    }
}
