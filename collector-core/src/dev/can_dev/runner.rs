use std::collections::HashMap;
use std::io;
use std::time::{Duration, Instant};

use futures::StreamExt;
use socketcan::{CanFrame, EmbeddedFrame, ExtendedId, Frame, Id, StandardId, tokio::CanSocket};
use tokio::sync::{mpsc, watch};
use tokio::time;
use tracing::{debug, info, warn};

use crate::center::SharedPointCenter;
use crate::config::can_conf::{
    ByteOrder, CanConfig, CanDataType, CanSignal, CanSignalConfig, CanSignalExtConfig, IdType,
};
use crate::core::point::{DataPoint, DownDataPoint, PointId, PointRef, Val};
use crate::dev::can_dev::CanDevError;
use crate::dev::{LifecycleState, dev_config::CanDeviceConfig, state::SharedState};

use super::backoff::Backoff;
use super::downlink::{FrameBinding, WritePlan, build_frame_map, build_name_map, build_point_map};
use crate::dev::can_bus::RawFrameRx;

pub(super) struct CanRunner {
    pub(super) id: String,
    pub(super) config: CanDeviceConfig,
    pub(super) configs: Vec<CanConfig>,
    pub(super) state: SharedState,
    pub(super) stop_rx: watch::Receiver<bool>,
    pub(super) rx: mpsc::Receiver<Vec<DownDataPoint>>,
    pub(super) raw_rx: RawFrameRx,
    pub(super) center: SharedPointCenter,
}

#[derive(Default)]
struct ExtSignalCache {
    values: HashMap<u32, ExtSignalState>,
}

struct ExtSignalState {
    values: Vec<Option<Val>>,
    filled: usize,
}

impl CanRunner {
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

    fn build_runtime_frame_map(&self) -> HashMap<u32, FrameState> {
        let mut states = HashMap::new();
        for cfg in self.configs.iter().filter(|cfg| cfg.frame.enable) {
            for raw_id in runtime_frame_ids(cfg) {
                states.entry(raw_id).or_insert_with(|| FrameState {
                    raw_id,
                    config: cfg.clone(),
                    last_seen: None,
                });
            }
        }
        states
    }

    fn connect(&self) -> Result<CanSocket, CanDevError> {
        if let Some(bitrate) = self.config.bitrate {
            self.setup_interface(bitrate)?;
        }
        Ok(CanSocket::open(&self.config.interface)?)
    }

    fn setup_interface(&self, bitrate: u32) -> Result<(), CanDevError> {
        // 先 down，忽略失败（接口可能已是 down 状态）
        let _ = std::process::Command::new("ip")
            .args(["link", "set", &self.config.interface, "down"])
            .output();

        let output = std::process::Command::new("ip")
            .args([
                "link",
                "set",
                &self.config.interface,
                "up",
                "type",
                "can",
                "bitrate",
                &bitrate.to_string(),
            ])
            .output()
            .map_err(|e| CanDevError::SetupInterface(e.to_string()))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(CanDevError::SetupInterface(stderr.trim().to_string()));
        }
        info!(
            "[{}] CAN接口已初始化: {} bitrate={}",
            self.id, self.config.interface, bitrate
        );
        Ok(())
    }

    fn decode_frame(
        frame: &CanFrame,
        states: &mut HashMap<u32, FrameState>,
        ext_cache: &mut ExtSignalCache,
        now: Instant,
    ) -> Vec<DataPoint> {
        let raw_id = frame.raw_id();
        let Some(state) = states.get_mut(&raw_id) else {
            return Vec::new();
        };
        if !matches_id_type(frame, state.config.frame.id_type) {
            return Vec::new();
        }
        state.last_seen = Some(now);
        let mut points = Vec::new();
        for signal in &state.config.signals {
            match signal {
                CanSignal::Normal(cfg) => {
                    if let Some(point) = decode_signal(cfg, frame.data()) {
                        points.push(point);
                    }
                }
                CanSignal::Ext(cfg) => {
                    if let Some(point) = decode_ext_signal(cfg, frame.data(), raw_id, ext_cache) {
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
        now: Instant,
        last_rx_at: Instant,
    ) -> Result<(), CanDevError> {
        if now.duration_since(last_rx_at) >= self.config.timeout {
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
            // 从未收到过的帧跳过：可能是下行帧（永远不会被接收），不做超时判断
            let Some(last_seen) = state.last_seen else {
                continue;
            };
            if now.duration_since(last_seen) >= timeout {
                return Err(CanDevError::Timeout(format!(
                    "报文0x{:X}超时{:?}",
                    state.raw_id, timeout
                )));
            }
        }
        Ok(())
    }

    async fn run_connected(
        &mut self,
        socket: &mut CanSocket,
        stop_rx: &mut watch::Receiver<bool>,
        frame_states: &mut HashMap<u32, FrameState>,
        point_map: &HashMap<u32, super::downlink::CanPointConfig>,
        name_map: &HashMap<&'static str, PointId>,
        frame_map: &HashMap<u32, FrameBinding>,
    ) -> Result<(), CanDevError> {
        self.state.store(&self.id, LifecycleState::Running);
        self.set_comm_fault(false);

        let connected_at = Instant::now();
        let mut last_rx_at = connected_at;
        let mut ticker = time::interval(self.config.interval);
        let mut ext_cache = ExtSignalCache::default();

        loop {
            tokio::select! {
                _ = stop_rx.changed() => {
                    if Self::stop_requested(stop_rx) {
                        self.set_comm_fault(true);
                        return Ok(());
                    }
                }
                _ = ticker.tick() => {
                    let now = Instant::now();
                    self.check_timeouts(frame_states, now, last_rx_at)?;
                }
                raw = self.raw_rx.recv() => {
                    if let Some((frame_id, data)) = raw {
                        if let Some(frame) = build_raw_frame(frame_id, &data) {
                            socket.write_frame(frame).await.map_err(CanDevError::WriteFrame)?;
                        } else {
                            warn!("[{}] Lua can.send: 无效 frame_id=0x{:X} 或数据长度错误", self.id, frame_id);
                        }
                    }
                }
                msg = self.rx.recv() => {
                    let Some(entries) = msg else {
                        self.state.store(&self.id, LifecycleState::Stopped);
                        self.set_comm_fault(true);
                        return Ok(());
                    };
                    let items: Vec<String> = entries.iter().map(|e| format!("{}: {}", resolve_signal_name(&e.point, point_map), e.value)).collect();
                    info!("[{}] ↓: {}", self.id, items.join(", "));
                    let plan =
                        WritePlan::build(entries, point_map, name_map, frame_map, self.center.as_ref(), &self.id);
                    if plan.is_empty() {
                        continue;
                    }
                    plan.apply(socket).await?;
                }
                result = socket.next() => {
                    let frame = result
                        .expect("CAN socket stream ended unexpectedly")
                        .map_err(|e| CanDevError::ReadFrame(io::Error::other(e)))?;
                    let now = Instant::now();
                    last_rx_at = now;

                    let mut batch: Vec<DataPoint> = Vec::new();

                    // 处理第一帧
                    let is_configured = frame_states.contains_key(&frame.raw_id());
                    let points = Self::decode_frame(&frame, frame_states, &mut ext_cache, now);
                    if points.is_empty() {
                        if !is_configured {
                            debug!("[{}] 忽略未配置CAN报文: 0x{:X}", self.id, frame.raw_id());
                        }
                    } else {
                        batch.extend(points);
                    }

                    // 不重新进入 epoll，直接排空内核缓冲区中已就绪的帧。
                    // 每批上限 64 帧，防止 CAN 错误帧风暴时长时间独占工作线程。
                    for _ in 0..64 {
                        match socket.try_read_frame() {
                            Ok(frame) => {
                                last_rx_at = now;
                                let is_configured = frame_states.contains_key(&frame.raw_id());
                                let points = Self::decode_frame(&frame, frame_states, &mut ext_cache, now);
                                if points.is_empty() {
                                    if !is_configured {
                                        debug!("[{}] 忽略未配置CAN报文: 0x{:X}", self.id, frame.raw_id());
                                    }
                                } else {
                                    batch.extend(points);
                                }
                            }
                            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                            Err(e) => {
                                if !batch.is_empty() {
                                    self.center.ingest(&self.id, batch);
                                }
                                return Err(CanDevError::ReadFrame(e));
                            }
                        }
                    }

                    if !batch.is_empty() {
                        self.center.ingest(&self.id, batch);
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
        let name_map = build_name_map(&self.configs);

        loop {
            if Self::stop_requested(&stop_rx) {
                self.state.store(&self.id, LifecycleState::Stopped);
                self.set_comm_fault(true);
                return;
            }

            self.state.store(&self.id, LifecycleState::Connecting);
            self.set_comm_fault(false);

            match self.connect() {
                Ok(mut socket) => {
                    self.state.store(&self.id, LifecycleState::Connected);
                    backoff.reset();

                    let mut frame_states = self.build_runtime_frame_map();
                    if let Err(err) = self
                        .run_connected(
                            &mut socket,
                            &mut stop_rx,
                            &mut frame_states,
                            &point_map,
                            &name_map,
                            &frame_map,
                        )
                        .await
                    {
                        self.state.store(&self.id, LifecycleState::Failed);
                        warn!("[{}] CAN连接中断，准备重连: {}", self.id, err);
                        self.set_comm_fault(true);
                    }
                }
                Err(err) => {
                    self.state.store(&self.id, LifecycleState::Failed);
                    warn!("[{}] 打开CAN接口失败，准备重连: {}", self.id, err);
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
                        self.set_comm_fault(true);
                        return;
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct FrameState {
    raw_id: u32,
    config: CanConfig,
    last_seen: Option<Instant>,
}

fn runtime_frame_ids(cfg: &CanConfig) -> Vec<u32> {
    let mut raw_ids = vec![cfg.frame.frame_id];
    for signal in &cfg.signals {
        let CanSignal::Ext(ext) = signal else {
            continue;
        };
        let frame_step = u32::from(ext.frame_id_step.max(1));
        for idx in 0..u32::from(ext.frame_num) {
            let raw_id = ext.frame_id + idx * frame_step;
            if !raw_ids.contains(&raw_id) {
                raw_ids.push(raw_id);
            }
        }
    }
    raw_ids
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
        key: cfg.name,
        translator: None,
        warn_bits: None,
        status_word: None,
        unit: None,
    })
}

fn decode_ext_signal(
    cfg: &CanSignalExtConfig,
    data: &[u8],
    raw_id: u32,
    ext_cache: &mut ExtSignalCache,
) -> Option<DataPoint> {
    let state = ext_cache
        .values
        .entry(cfg.id)
        .or_insert_with(|| ExtSignalState {
            values: vec![None; usize::from(cfg.total_element)],
            filled: 0,
        });

    let frame_step = u32::from(cfg.frame_id_step.max(1));
    let frame_offset = raw_id.checked_sub(cfg.frame_id)? / frame_step;
    let element_offset = usize::try_from(frame_offset).ok()? * usize::from(cfg.each_frame_element);

    if element_offset == 0 {
        state.values.fill(None);
        state.filled = 0;
    }

    if !decode_ext_segment_into(cfg, data, element_offset, state) {
        return None;
    }

    if state.filled != state.values.len() {
        return None;
    }

    Some(DataPoint {
        id: cfg.id,
        name: cfg.name,
        value: Val::List(
            state
                .values
                .iter()
                .map(|value| {
                    value
                        .clone()
                        .expect("ext signal cache should be fully populated before publishing")
                })
                .collect(),
        ),
        key: "",
        translator: None,
        warn_bits: None,
        status_word: None,
        unit: None,
    })
}

fn decode_ext_segment_into(
    cfg: &CanSignalExtConfig,
    data: &[u8],
    element_offset: usize,
    state: &mut ExtSignalState,
) -> bool {
    let mut wrote_value = false;
    for idx in 0..usize::from(cfg.each_frame_element) {
        if element_offset + idx >= usize::from(cfg.total_element) {
            break;
        }
        let start_bit =
            usize::from(cfg.element_start_bit) + idx * usize::from(cfg.single_ele_bit_len);
        let Some(start_bit) = u8::try_from(start_bit).ok() else {
            return false;
        };
        let Some(raw) = extract_raw(data, start_bit, cfg.single_ele_bit_len, cfg.byte_order) else {
            return false;
        };
        if cfg.invalid_val.is_some_and(|invalid| invalid == raw) {
            continue;
        }
        let target_idx = element_offset + idx;
        if state.values[target_idx].is_none() {
            state.filled += 1;
        }
        state.values[target_idx] = Some(decode_value(
            raw,
            cfg.single_ele_bit_len,
            cfg.data_type,
            cfg.scale,
            cfg.offset,
        ));
        wrote_value = true;
    }
    wrote_value
}

pub(super) fn extract_raw(
    data: &[u8],
    start_bit: u8,
    bit_len: u8,
    byte_order: ByteOrder,
) -> Option<u32> {
    if bit_len == 0 || bit_len > 32 {
        return None;
    }
    match byte_order {
        ByteOrder::Intel => extract_intel(data, start_bit, bit_len),
        ByteOrder::Motorola => extract_motorola(data, start_bit, bit_len),
    }
}

fn extract_intel(data: &[u8], start_bit: u8, bit_len: u8) -> Option<u32> {
    let start = usize::from(start_bit);
    let end_byte = (start + usize::from(bit_len) - 1) / 8;
    if end_byte >= data.len() {
        return None;
    }
    // 将覆盖范围内的字节打包进 u64，一次位移+掩码完成提取
    let start_byte = start / 8;
    let mut val = 0u64;
    for (i, &b) in data[start_byte..=end_byte].iter().enumerate() {
        val |= (b as u64) << (i * 8);
    }
    let shift = start % 8;
    let mask = if bit_len < 32 {
        (1u64 << bit_len) - 1
    } else {
        u32::MAX as u64
    };
    Some(((val >> shift) & mask) as u32)
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

fn round_scaled(value: f64, scale: f64) -> f64 {
    // 根据 scale 的小数位数四舍五入，消除浮点乘法噪声（如 239 * 0.001 = 0.239000...02）
    let decimals = if scale > 0.0 && scale < 1.0 {
        (-scale.log10().floor()) as i32 + 1
    } else {
        6
    };
    let factor = 10f64.powi(decimals);
    (value * factor).round() / factor
}

fn decode_value(raw: u32, bit_len: u8, data_type: CanDataType, scale: f64, offset: f64) -> Val {
    let needs_scale = (scale - 1.0).abs() > f64::EPSILON || offset.abs() > f64::EPSILON;
    match data_type {
        CanDataType::U8 => {
            let value = raw as u8;
            if needs_scale {
                Val::F64(round_scaled(f64::from(value) * scale + offset, scale))
            } else {
                Val::U8(value)
            }
        }
        CanDataType::U16 => {
            let value = raw as u16;
            if needs_scale {
                Val::F64(round_scaled(f64::from(value) * scale + offset, scale))
            } else {
                Val::U16(value)
            }
        }
        CanDataType::U32 => {
            if needs_scale {
                Val::F64(round_scaled(raw as f64 * scale + offset, scale))
            } else {
                Val::U32(raw)
            }
        }
        CanDataType::I16 => {
            let value = sign_extend(raw, bit_len) as i16;
            if needs_scale {
                Val::F64(round_scaled(f64::from(value) * scale + offset, scale))
            } else {
                Val::I16(value)
            }
        }
        CanDataType::I32 => {
            let value = sign_extend(raw, bit_len);
            if needs_scale {
                Val::F64(round_scaled(value as f64 * scale + offset, scale))
            } else {
                Val::I32(value)
            }
        }
    }
}

fn build_raw_frame(frame_id: u32, data: &[u8]) -> Option<CanFrame> {
    let id: Id = if frame_id <= 0x7FF {
        StandardId::new(frame_id as u16)?.into()
    } else {
        ExtendedId::new(frame_id)?.into()
    };
    CanFrame::new(id, data)
}

fn sign_extend(raw: u32, bit_len: u8) -> i32 {
    if bit_len == 0 || bit_len >= 32 {
        return raw as i32;
    }
    let shift = 32 - u32::from(bit_len);
    ((raw << shift) as i32) >> shift
}

fn resolve_signal_name<'a>(
    point: &'a PointRef,
    point_map: &'a HashMap<PointId, super::downlink::CanPointConfig>,
) -> &'a str {
    match point {
        PointRef::Key(k) | PointRef::Name(k) => k,
        PointRef::Id(id) => point_map
            .get(id)
            .map(|cfg| match cfg.signal {
                CanSignal::Normal(s) => s.name,
                CanSignal::Ext(s) => s.name,
            })
            .unwrap_or("unknown"),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{ExtSignalCache, decode_ext_signal, runtime_frame_ids};
    use crate::config::can_conf::{
        ByteOrder, CanConfig, CanDataType, CanFrameConfig, CanSignal, CanSignalConfig,
        CanSignalExtConfig, IdType, Rule,
    };
    use crate::core::point::Val;

    #[test]
    fn runtime_frame_ids_include_extended_sequence_frames() {
        let cfg = CanConfig {
            frame: CanFrameConfig {
                id: 1,
                name: "frame",
                frame_id: 0x100,
                id_type: IdType::Extended,
                dlc: 8,
                cycle_duration: Duration::from_millis(100),
                timeout_duration: Duration::from_millis(200),
                send: "a",
                receive: "b",
                rule: Rule::Cycle,
                enable: true,
            },
            signals: vec![
                CanSignal::Normal(CanSignalConfig {
                    id: 10,
                    name: "speed",
                    frame_id: 0x100,
                    signal_name: "speed",
                    start_bit: 0,
                    bit_len: 16,
                    byte_order: ByteOrder::Intel,
                    data_type: CanDataType::U16,
                    scale: 1.0,
                    offset: 0.0,
                    unit: "",
                    invalid_val: None,
                    enum_values: "",
                }),
                CanSignal::Ext(CanSignalExtConfig {
                    id: 11,
                    name: "cells",
                    poly_name: "cells",
                    frame_id: 0x100,
                    frame_num: 3,
                    frame_id_step: 2,
                    each_frame_element: 4,
                    total_element: 12,
                    element_start_bit: 0,
                    single_ele_bit_len: 8,
                    byte_order: ByteOrder::Intel,
                    data_type: CanDataType::U8,
                    scale: 1.0,
                    offset: 0.0,
                    unit: "",
                    invalid_val: None,
                }),
            ],
        };

        let raw_ids = runtime_frame_ids(&cfg);

        assert_eq!(raw_ids, vec![0x100, 0x102, 0x104]);
    }

    #[test]
    fn decode_ext_signal_uses_frame_offset_for_later_frames() {
        let cfg = CanSignalExtConfig {
            id: 11,
            name: "cells",
            poly_name: "cells",
            frame_id: 0x100,
            frame_num: 3,
            frame_id_step: 2,
            each_frame_element: 4,
            total_element: 8,
            element_start_bit: 0,
            single_ele_bit_len: 8,
            byte_order: ByteOrder::Intel,
            data_type: CanDataType::U8,
            scale: 1.0,
            offset: 0.0,
            unit: "",
            invalid_val: None,
        };

        let mut cache = ExtSignalCache::default();
        assert!(decode_ext_signal(&cfg, &[1, 2, 3, 4], 0x100, &mut cache).is_none());
        let point = decode_ext_signal(&cfg, &[5, 6, 7, 8], 0x102, &mut cache).expect("point");

        assert_eq!(
            point.value,
            Val::List(vec![
                Val::U8(1),
                Val::U8(2),
                Val::U8(3),
                Val::U8(4),
                Val::U8(5),
                Val::U8(6),
                Val::U8(7),
                Val::U8(8),
            ])
        );
    }

    #[test]
    fn decode_ext_signal_resets_cache_when_new_cycle_starts() {
        let cfg = CanSignalExtConfig {
            id: 11,
            name: "cells",
            poly_name: "cells",
            frame_id: 0x100,
            frame_num: 2,
            frame_id_step: 2,
            each_frame_element: 4,
            total_element: 8,
            element_start_bit: 0,
            single_ele_bit_len: 8,
            byte_order: ByteOrder::Intel,
            data_type: CanDataType::U8,
            scale: 1.0,
            offset: 0.0,
            unit: "",
            invalid_val: None,
        };

        let mut cache = ExtSignalCache::default();
        assert!(decode_ext_signal(&cfg, &[1, 2, 3, 4], 0x100, &mut cache).is_none());
        let first_cycle =
            decode_ext_signal(&cfg, &[5, 6, 7, 8], 0x102, &mut cache).expect("first cycle");
        assert_eq!(
            first_cycle.value,
            Val::List(vec![
                Val::U8(1),
                Val::U8(2),
                Val::U8(3),
                Val::U8(4),
                Val::U8(5),
                Val::U8(6),
                Val::U8(7),
                Val::U8(8),
            ])
        );

        assert!(decode_ext_signal(&cfg, &[11, 12, 13, 14], 0x100, &mut cache).is_none());
        let second_cycle =
            decode_ext_signal(&cfg, &[15, 16, 17, 18], 0x102, &mut cache).expect("second cycle");
        assert_eq!(
            second_cycle.value,
            Val::List(vec![
                Val::U8(11),
                Val::U8(12),
                Val::U8(13),
                Val::U8(14),
                Val::U8(15),
                Val::U8(16),
                Val::U8(17),
                Val::U8(18),
            ])
        );
    }
}
