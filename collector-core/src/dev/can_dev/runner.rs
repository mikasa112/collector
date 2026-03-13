use std::collections::HashMap;
use std::time::{Duration, Instant};

use socketcan::{CanFrame, EmbeddedFrame, Frame, tokio::CanSocket};
use tokio::sync::{mpsc, watch};
use tokio::time;
use tracing::{debug, info, warn};

use crate::center::{Center, global_center};
use crate::config::can_conf::{
    ByteOrder, CanConfig, CanDataType, CanSignal, CanSignalConfig, CanSignalExtConfig, IdType,
};
use crate::core::point::{DataPoint, DataPoints, Val};
use crate::dev::can_dev::CanDevError;
use crate::dev::{Identifiable, LifecycleState, dev_config::CanDeviceConfig, state::SharedState};

use super::backoff::Backoff;
use super::downlink::{FrameBinding, WritePlan, build_frame_map, build_point_map};

pub(super) struct CanRunner {
    pub(super) id: String,
    pub(super) config: CanDeviceConfig,
    pub(super) configs: Vec<CanConfig>,
    pub(super) state: SharedState,
    pub(super) stop_rx: watch::Receiver<bool>,
    pub(super) rx: mpsc::Receiver<Vec<DataPoint>>,
}

impl Identifiable for CanRunner {
    fn id(&self) -> &str {
        &self.id
    }
}

impl CanRunner {
    fn report_comm_status(&self, v: u8) {
        global_center().ingest(
            self,
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

    fn build_runtime_frame_map(&self) -> HashMap<u32, FrameState> {
        self.configs
            .iter()
            .filter(|cfg| cfg.frame.enable)
            .map(|cfg| {
                (
                    cfg.frame.frame_id,
                    FrameState {
                        config: cfg.clone(),
                        last_seen: None,
                    },
                )
            })
            .collect()
    }

    fn connect(&self) -> Result<CanSocket, CanDevError> {
        Ok(CanSocket::open(&self.config.interface)?)
    }

    fn decode_frame(frame: &CanFrame, states: &mut HashMap<u32, FrameState>) -> Vec<DataPoint> {
        let raw_id = frame.raw_id();
        let Some(state) = states.get_mut(&raw_id) else {
            return Vec::new();
        };
        if !matches_id_type(frame, state.config.frame.id_type) {
            return Vec::new();
        }
        state.last_seen = Some(Instant::now());
        let mut points = Vec::new();
        for signal in &state.config.signals {
            match signal {
                CanSignal::Normal(cfg) => {
                    if let Some(point) = decode_signal(cfg, frame.data()) {
                        points.push(point);
                    }
                }
                CanSignal::Ext(cfg) => {
                    if let Some(point) = decode_ext_signal(cfg, frame.data(), raw_id) {
                        points.push(point);
                    }
                }
            }
        }
        points
    }

    fn check_timeouts(
        &self,
        states: &HashMap<u32, FrameState>,
        connected_at: Instant,
        last_rx_at: Instant,
    ) -> Result<(), CanDevError> {
        if last_rx_at.elapsed() >= self.config.timeout {
            return Err(CanDevError::Timeout(format!(
                "接口{}在{:?}内未收到CAN报文",
                self.config.interface, self.config.timeout
            )));
        }

        for state in states.values() {
            let timeout = state.config.frame.timeout_duration;
            if timeout.is_zero() {
                continue;
            }
            let elapsed = state
                .last_seen
                .map(|instant| instant.elapsed())
                .unwrap_or_else(|| connected_at.elapsed());
            if elapsed >= timeout {
                return Err(CanDevError::Timeout(format!(
                    "报文0x{:X}超时{:?}",
                    state.config.frame.frame_id, timeout
                )));
            }
        }
        Ok(())
    }

    async fn run_connected(
        &mut self,
        socket: &CanSocket,
        stop_rx: &mut watch::Receiver<bool>,
        frame_states: &mut HashMap<u32, FrameState>,
        point_map: &HashMap<u32, super::downlink::CanPointConfig>,
        frame_map: &HashMap<u32, FrameBinding>,
    ) -> Result<(), CanDevError> {
        self.state.store(&self.id, LifecycleState::Running);
        self.report_comm_status(1);

        let connected_at = Instant::now();
        let mut last_rx_at = connected_at;
        let mut ticker = time::interval(self.config.interval);

        loop {
            tokio::select! {
                _ = stop_rx.changed() => {
                    if Self::stop_requested(stop_rx) {
                        self.report_comm_status(0);
                        return Ok(());
                    }
                }
                _ = ticker.tick() => {
                    self.check_timeouts(frame_states, connected_at, last_rx_at)?;
                }
                msg = self.rx.recv() => {
                    let Some(entries) = msg else {
                        self.state.store(&self.id, LifecycleState::Stopped);
                        self.report_comm_status(0);
                        return Ok(());
                    };
                    let points = DataPoints(entries.clone());
                    info!("[{}] ↓: {}", self.id, points);
                    let plan = WritePlan::build(entries, point_map, frame_map, self);
                    if plan.is_empty() {
                        continue;
                    }
                    plan.apply(socket).await?;
                }
                result = socket.read_frame() => {
                    let frame = result.map_err(CanDevError::ReadFrame)?;
                    last_rx_at = Instant::now();
                    let points = Self::decode_frame(&frame, frame_states);
                    if !points.is_empty() {
                        // info!("[{}] ↑: {}", self.id, DataPoints(points.clone()));
                        global_center().ingest(self, points);
                    } else {
                        debug!("[{}] 忽略未配置CAN报文: 0x{:X}", self.id, frame.raw_id());
                    }
                }
            }
        }
    }

    pub(super) async fn run(mut self) {
        let mut backoff = Backoff::new(Duration::from_millis(500), Duration::from_secs(10));
        let mut stop_rx = self.stop_rx.clone();
        let point_map = build_point_map(&self.configs);
        let frame_map = build_frame_map(&self.configs);

        loop {
            if Self::stop_requested(&stop_rx) {
                self.state.store(&self.id, LifecycleState::Stopped);
                self.report_comm_status(0);
                return;
            }

            self.state.store(&self.id, LifecycleState::Connecting);
            self.report_comm_status(0);

            match self.connect() {
                Ok(socket) => {
                    self.state.store(&self.id, LifecycleState::Connected);
                    backoff.reset();

                    let mut frame_states = self.build_runtime_frame_map();
                    if let Err(err) = self
                        .run_connected(
                            &socket,
                            &mut stop_rx,
                            &mut frame_states,
                            &point_map,
                            &frame_map,
                        )
                        .await
                    {
                        self.state.store(&self.id, LifecycleState::Failed);
                        warn!("[{}] CAN连接中断，准备重连: {}", self.id, err);
                        self.report_comm_status(0);
                    }
                }
                Err(err) => {
                    self.state.store(&self.id, LifecycleState::Failed);
                    warn!("[{}] 打开CAN接口失败，准备重连: {}", self.id, err);
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
                        self.report_comm_status(0);
                        return;
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct FrameState {
    config: CanConfig,
    last_seen: Option<Instant>,
}

fn matches_id_type(frame: &CanFrame, id_type: IdType) -> bool {
    match id_type {
        IdType::Standard => !frame.is_extended(),
        IdType::Extended => frame.is_extended(),
    }
}

fn decode_signal(cfg: &CanSignalConfig, data: &[u8]) -> Option<DataPoint> {
    let raw = extract_raw(data, cfg.start_bit, cfg.bit_len, cfg.byte_order)?;
    if cfg.invalid_val.is_some_and(|invalid| invalid == raw) {
        return None;
    }
    Some(DataPoint {
        id: cfg.id,
        name: cfg.name,
        value: decode_value(raw, cfg.bit_len, cfg.data_type, cfg.scale, cfg.offset),
    })
}

fn decode_ext_signal(cfg: &CanSignalExtConfig, data: &[u8], raw_id: u32) -> Option<DataPoint> {
    let frame_step = u32::from(cfg.frame_id_step.max(1));
    let frame_offset = raw_id.checked_sub(cfg.frame_id)? / frame_step;
    let element_offset = usize::try_from(frame_offset).ok()? * usize::from(cfg.each_frame_element);
    let mut values = Vec::new();

    for idx in 0..usize::from(cfg.each_frame_element) {
        if element_offset + idx >= usize::from(cfg.total_element) {
            break;
        }
        let start_bit =
            usize::from(cfg.element_start_bit) + idx * usize::from(cfg.single_ele_bit_len);
        let raw = extract_raw(
            data,
            u8::try_from(start_bit).ok()?,
            cfg.single_ele_bit_len,
            cfg.byte_order,
        )?;
        if cfg.invalid_val.is_some_and(|invalid| invalid == raw) {
            continue;
        }
        values.push(decode_value(
            raw,
            cfg.single_ele_bit_len,
            cfg.data_type,
            cfg.scale,
            cfg.offset,
        ));
    }

    if values.is_empty() {
        return None;
    }

    Some(DataPoint {
        id: cfg.id,
        name: cfg.name,
        value: Val::List(values),
    })
}

fn extract_raw(data: &[u8], start_bit: u8, bit_len: u8, byte_order: ByteOrder) -> Option<u32> {
    if bit_len == 0 || bit_len > 32 {
        return None;
    }
    match byte_order {
        ByteOrder::Intel => extract_intel(data, start_bit, bit_len),
        ByteOrder::Motorola => extract_motorola(data, start_bit, bit_len),
    }
}

fn extract_intel(data: &[u8], start_bit: u8, bit_len: u8) -> Option<u32> {
    let mut raw = 0u32;
    for bit_idx in 0..usize::from(bit_len) {
        let pos = usize::from(start_bit) + bit_idx;
        let byte = *data.get(pos / 8)?;
        let bit = (byte >> (pos % 8)) & 1;
        raw |= u32::from(bit) << bit_idx;
    }
    Some(raw)
}

fn extract_motorola(data: &[u8], start_bit: u8, bit_len: u8) -> Option<u32> {
    let mut raw = 0u32;
    let mut pos = i32::from(start_bit);
    for _ in 0..usize::from(bit_len) {
        let pos_usize = usize::try_from(pos).ok()?;
        let byte = *data.get(pos_usize / 8)?;
        let bit = (byte >> (pos_usize % 8)) & 1;
        raw = (raw << 1) | u32::from(bit);
        pos = if pos % 8 == 0 { pos + 15 } else { pos - 1 };
    }
    Some(raw)
}

fn decode_value(raw: u32, bit_len: u8, data_type: CanDataType, scale: f32, offset: f32) -> Val {
    let needs_scale = (scale - 1.0).abs() > f32::EPSILON || offset.abs() > f32::EPSILON;
    match data_type {
        CanDataType::U8 => {
            let value = raw as u8;
            if needs_scale {
                Val::F32(f32::from(value) * scale + offset)
            } else {
                Val::U8(value)
            }
        }
        CanDataType::U16 => {
            let value = raw as u16;
            if needs_scale {
                Val::F32(f32::from(value) * scale + offset)
            } else {
                Val::U16(value)
            }
        }
        CanDataType::U32 => {
            if needs_scale {
                Val::F32(raw as f32 * scale + offset)
            } else {
                Val::U32(raw)
            }
        }
        CanDataType::I16 => {
            let value = sign_extend(raw, bit_len) as i16;
            if needs_scale {
                Val::F32(f32::from(value) * scale + offset)
            } else {
                Val::I16(value)
            }
        }
        CanDataType::I32 => {
            let value = sign_extend(raw, bit_len);
            if needs_scale {
                Val::F32(value as f32 * scale + offset)
            } else {
                Val::I32(value)
            }
        }
    }
}

fn sign_extend(raw: u32, bit_len: u8) -> i32 {
    if bit_len == 0 || bit_len >= 32 {
        return raw as i32;
    }
    let shift = 32 - u32::from(bit_len);
    ((raw << shift) as i32) >> shift
}
