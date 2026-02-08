use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::time::Duration;

use tokio::sync::watch;
use tokio::time;
use tokio_modbus::Slave;
use tokio_modbus::client::{Context, Reader, rtu, tcp};
use tokio_modbus::prelude::SlaveContext;
use tokio_serial::{DataBits, Parity};
use tracing::warn;

use crate::center::data_center::Entry;
use crate::center::{Center, global_center};
use crate::config::modbus_conf::{
    ByteOrder, ModbusConfig, ModbusConfigs, ModbusDataType, RegisterType,
};
use crate::core::point::Val;
use crate::dev::modbus_dev::Protocol;
use crate::dev::{Identifiable, LifecycleState};

use super::backoff::Backoff;
use super::batch::{ReadBatch, range_end, register_type_key};
use super::error::ModbusDevError;
use super::state::store_state;

pub(super) struct ModbusRunner {
    pub(super) id: String,
    pub(super) protocol: Protocol,
    pub(super) configs: ModbusConfigs,
    pub(super) state: Arc<AtomicU8>,
    pub(super) stop_rx: watch::Receiver<bool>,
}

impl Identifiable for ModbusRunner {
    fn id(&self) -> String {
        return self.id.clone();
    }
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
                    match self.read_all(ctx).await {
                        Ok(entries) => {
                            if !entries.is_empty() {
                                global_center().ingest(self, entries);
                            }
                        }
                        Err(err) => {
                            warn!("[{}] 读取失败, 准备重连: {}", self.id, err);
                            return;
                        }
                    }
                }
            }
        }
    }

    pub(super) async fn run(&self) {
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

    fn qty_u16(&self, qty: usize) -> Option<u16> {
        if qty == 0 || qty > u16::MAX as usize {
            return None;
        }
        Some(qty as u16)
    }

    async fn read_all(&self, ctx: &mut Context) -> Result<Vec<Entry>, ModbusDevError> {
        let mut entries = Vec::with_capacity(self.configs.len());
        let mut cfgs: Vec<&ModbusConfig> = self.configs.iter().collect();
        cfgs.sort_by_key(|c| (register_type_key(c.register_type), c.register_address));

        let batches = self.build_batches(&cfgs);
        for batch in batches {
            self.read_batch(ctx, &batch, &mut entries).await?;
        }
        Ok(entries)
    }

    fn build_entry_from_bools(&self, cfg: &ModbusConfig, vals: &[bool]) -> Option<Entry> {
        let v = vals.first().copied().unwrap_or(false);
        Some(Entry {
            key: cfg.name.clone(),
            value: Val::U8(if v { 1 } else { 0 }),
        })
    }

    fn build_entry_from_regs(&self, cfg: &ModbusConfig, vals: &[u16]) -> Option<Entry> {
        let val = match cfg.data_type {
            ModbusDataType::Bool => {
                let v = vals.first().copied().unwrap_or(0);
                Val::U8(if v != 0 { 1 } else { 0 })
            }
            ModbusDataType::U16 => {
                let v = *vals.first()?;
                let v = self.apply_u16_byte_order(v, cfg.byte_order);
                self.apply_scale_u16(v, cfg.scale, cfg.offset)
            }
            ModbusDataType::I16 => {
                let v = *vals.first()?;
                let v = self.apply_u16_byte_order(v, cfg.byte_order);
                let v = v as i16;
                self.apply_scale_i16(v, cfg.scale, cfg.offset)
            }
            ModbusDataType::U32 => {
                if vals.len() < 2 {
                    warn!("[{}] 点位{}数量不足, 需要2", self.id, cfg.name);
                    return None;
                }
                let v = self.apply_u32_byte_order(vals[0], vals[1], cfg.byte_order);
                self.apply_scale_u32(v, cfg.scale, cfg.offset)
            }
            ModbusDataType::I32 => {
                if vals.len() < 2 {
                    warn!("[{}] 点位{}数量不足, 需要2", self.id, cfg.name);
                    return None;
                }
                let v = self.apply_u32_byte_order(vals[0], vals[1], cfg.byte_order) as i32;
                self.apply_scale_i32(v, cfg.scale, cfg.offset)
            }
        };
        Some(Entry {
            key: cfg.name.clone(),
            value: val,
        })
    }

    fn apply_u16_byte_order(&self, v: u16, order: Option<ByteOrder>) -> u16 {
        let [hi, lo] = v.to_be_bytes();
        let bytes = match order {
            Some(ByteOrder::BA) => [lo, hi],
            _ => [hi, lo],
        };
        u16::from_be_bytes(bytes)
    }

    fn apply_u32_byte_order(&self, r0: u16, r1: u16, order: Option<ByteOrder>) -> u32 {
        let [a, b] = r0.to_be_bytes();
        let [c, d] = r1.to_be_bytes();
        let bytes = match order {
            Some(ByteOrder::CDAB) => [c, d, a, b],
            Some(ByteOrder::BA) => [b, a, d, c],
            _ => [a, b, c, d],
        };
        u32::from_be_bytes(bytes)
    }

    fn apply_scale_u16(&self, v: u16, scale: f64, offset: f64) -> Val {
        if scale == 1.0 && offset == 0.0 {
            return Val::U16(v);
        }
        Val::F32((v as f64 * scale + offset) as f32)
    }

    fn apply_scale_i16(&self, v: i16, scale: f64, offset: f64) -> Val {
        if scale == 1.0 && offset == 0.0 {
            return Val::I16(v);
        }
        Val::F32((v as f64 * scale + offset) as f32)
    }

    fn apply_scale_u32(&self, v: u32, scale: f64, offset: f64) -> Val {
        if scale == 1.0 && offset == 0.0 {
            return Val::U32(v);
        }
        Val::F32((v as f64 * scale + offset) as f32)
    }

    fn apply_scale_i32(&self, v: i32, scale: f64, offset: f64) -> Val {
        if scale == 1.0 && offset == 0.0 {
            return Val::I32(v);
        }
        Val::F32((v as f64 * scale + offset) as f32)
    }

    fn entry_from_bool_batch(
        &self,
        cfg: &ModbusConfig,
        start: usize,
        vals: &[bool],
    ) -> Option<Entry> {
        let offset = cfg.register_address as usize - start;
        let end = offset + cfg.quantity;
        if end > vals.len() {
            warn!("[{}] 点位{}数量不足", self.id, cfg.name);
            return None;
        }
        self.build_entry_from_bools(cfg, &vals[offset..end])
    }

    fn entry_from_reg_batch(
        &self,
        cfg: &ModbusConfig,
        start: usize,
        vals: &[u16],
    ) -> Option<Entry> {
        let offset = cfg.register_address as usize - start;
        let end = offset + cfg.quantity;
        if end > vals.len() {
            warn!("[{}] 点位{}数量不足", self.id, cfg.name);
            return None;
        }
        self.build_entry_from_regs(cfg, &vals[offset..end])
    }

    fn max_batch_for(&self, rt: RegisterType) -> usize {
        match rt {
            RegisterType::Coils | RegisterType::DiscreteInputs => 2000,
            RegisterType::HoldingRegisters | RegisterType::InputRegisters => 125,
        }
    }

    fn build_batches<'a>(&self, cfgs: &'a [&ModbusConfig]) -> Vec<ReadBatch<'a>> {
        let mut batches = Vec::new();
        let mut i = 0usize;
        while i < cfgs.len() {
            let first = cfgs[i];
            let max_batch = self.max_batch_for(first.register_type);
            if first.quantity > max_batch {
                warn!(
                    "[{}] 点位{}数量过大(>{}), 已跳过",
                    self.id, first.name, max_batch
                );
                i += 1;
                continue;
            }
            let rt = first.register_type;
            let start = first.register_address as usize;
            let mut end = match range_end(start, first.quantity) {
                Some(v) => v,
                None => {
                    warn!("[{}] 无效数量: {}", self.id, first.quantity);
                    i += 1;
                    continue;
                }
            };
            let mut batch = vec![first];
            let mut j = i + 1;
            while j < cfgs.len() {
                let cfg = cfgs[j];
                if cfg.register_type != rt {
                    break;
                }
                let cfg_start = cfg.register_address as usize;
                if cfg_start > end {
                    break;
                }
                if cfg.quantity > max_batch {
                    warn!(
                        "[{}] 点位{}数量过大(>{}), 已跳过",
                        self.id, cfg.name, max_batch
                    );
                    j += 1;
                    continue;
                }
                let cfg_end = match range_end(cfg_start, cfg.quantity) {
                    Some(v) => v,
                    None => {
                        warn!("[{}] 无效数量: {}", self.id, cfg.quantity);
                        j += 1;
                        continue;
                    }
                };
                if cfg_end - start > max_batch {
                    break;
                }
                if cfg_end > end {
                    end = cfg_end;
                }
                batch.push(cfg);
                j += 1;
            }
            i = j;
            if start >= end {
                continue;
            }
            batches.push(ReadBatch {
                register_type: rt,
                start,
                end,
                configs: batch,
            });
        }
        batches
    }

    async fn read_batch(
        &self,
        ctx: &mut Context,
        batch: &ReadBatch<'_>,
        entries: &mut Vec<Entry>,
    ) -> Result<(), ModbusDevError> {
        let qty = match self.qty_u16(batch.end - batch.start) {
            Some(v) => v,
            None => {
                warn!("[{}] 无效读取范围: {}..{}", self.id, batch.start, batch.end);
                return Ok(());
            }
        };

        match batch.register_type {
            RegisterType::Coils => {
                let vals = ctx.read_coils(batch.start as u16, qty).await??;
                for cfg in &batch.configs {
                    if let Some(entry) = self.entry_from_bool_batch(cfg, batch.start, &vals) {
                        entries.push(entry);
                    }
                }
            }
            RegisterType::DiscreteInputs => {
                let vals = ctx.read_discrete_inputs(batch.start as u16, qty).await??;
                for cfg in &batch.configs {
                    if let Some(entry) = self.entry_from_bool_batch(cfg, batch.start, &vals) {
                        entries.push(entry);
                    }
                }
            }
            RegisterType::HoldingRegisters => {
                let vals = ctx
                    .read_holding_registers(batch.start as u16, qty)
                    .await??;
                for cfg in &batch.configs {
                    if let Some(entry) = self.entry_from_reg_batch(cfg, batch.start, &vals) {
                        entries.push(entry);
                    }
                }
            }
            RegisterType::InputRegisters => {
                let vals = ctx.read_input_registers(batch.start as u16, qty).await??;
                for cfg in &batch.configs {
                    if let Some(entry) = self.entry_from_reg_batch(cfg, batch.start, &vals) {
                        entries.push(entry);
                    }
                }
            }
        }
        Ok(())
    }
}
