use std::collections::{BTreeMap, HashMap, HashSet};

use socketcan::{CanFrame, EmbeddedFrame, ExtendedId, Id, StandardId};
use tracing::warn;

use crate::center::PointCenter;
use crate::config::can_conf::{
    ByteOrder, CanConfig, CanDataType, CanFrameConfig, CanSignal, IdType,
};
use crate::core::point::{DataPoint, PointId, Val};
use crate::dev::can_dev::CanDevError;

pub(super) struct WritePlan {
    frames: Vec<CanFrame>,
}

impl WritePlan {
    pub(super) fn build(
        entries: Vec<DataPoint>,
        point_map: &HashMap<PointId, CanPointConfig>,
        frame_map: &HashMap<u32, FrameBinding>,
        center: &dyn PointCenter,
        dev_id: &str,
    ) -> Self {
        let mut payloads: BTreeMap<u32, FramePayload> = BTreeMap::new();
        let mut initialized_bindings: HashSet<u32> = HashSet::new();

        for entry in &entries {
            let Some(point_cfg) = point_map.get(&entry.id) else {
                warn!("[{}] 未找到点位配置, 忽略CAN下发: {}", dev_id, entry.id);
                continue;
            };
            let Some(binding) = frame_map.get(&point_cfg.binding_frame_id) else {
                warn!("[{}] 未找到报文配置, 忽略CAN下发: {}", dev_id, entry.id);
                continue;
            };
            if initialized_bindings.insert(binding.frame.frame_id) {
                preload_frame_payloads(center, &mut payloads, binding, point_map, dev_id);
            }
        }

        for entry in entries {
            let Some(point_cfg) = point_map.get(&entry.id) else {
                continue;
            };
            encode_entry(&mut payloads, point_cfg, &entry.value, dev_id);
        }

        WritePlan {
            frames: payloads
                .into_values()
                .filter_map(|payload| payload.build_frame(dev_id))
                .collect(),
        }
    }

    pub(super) async fn apply(
        &self,
        socket: &socketcan::tokio::CanSocket,
    ) -> Result<(), CanDevError> {
        for frame in &self.frames {
            socket
                .write_frame(*frame)
                .await
                .map_err(CanDevError::WriteFrame)?;
        }
        Ok(())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }
}

#[derive(Clone, Copy)]
pub(super) struct CanPointConfig {
    binding_frame_id: u32,
    frame: CanFrameConfig,
    signal: CanSignal,
}

#[derive(Clone)]
pub(super) struct FrameBinding {
    pub(super) frame: CanFrameConfig,
    point_ids: Vec<PointId>,
}

#[derive(Clone)]
struct FramePayload {
    frame: CanFrameConfig,
    raw_id: u32,
    data: Vec<u8>,
}

impl FramePayload {
    fn new(frame: CanFrameConfig, raw_id: u32) -> Self {
        Self {
            frame,
            raw_id,
            data: vec![0; usize::from(frame.dlc)],
        }
    }

    fn build_frame(self, dev_id: &str) -> Option<CanFrame> {
        let id: Id = match self.frame.id_type {
            IdType::Standard => StandardId::new(self.raw_id as u16)?.into(),
            IdType::Extended => ExtendedId::new(self.raw_id)?.into(),
        };
        CanFrame::new(id, &self.data).or_else(|| {
            warn!(
                "[{}] 构建CAN报文失败, frame_id=0x{:X}, dlc={}",
                dev_id, self.raw_id, self.frame.dlc
            );
            None
        })
    }
}

pub(super) fn build_point_map(configs: &[CanConfig]) -> HashMap<PointId, CanPointConfig> {
    let mut map = HashMap::new();
    for cfg in configs.iter().filter(|cfg| cfg.frame.enable) {
        for signal in &cfg.signals {
            let id = match signal {
                CanSignal::Normal(signal) => signal.id,
                CanSignal::Ext(signal) => signal.id,
            };
            map.insert(
                id,
                CanPointConfig {
                    binding_frame_id: cfg.frame.frame_id,
                    frame: cfg.frame,
                    signal: *signal,
                },
            );
        }
    }
    map
}

pub(super) fn build_frame_map(configs: &[CanConfig]) -> HashMap<u32, FrameBinding> {
    let mut map = HashMap::new();
    for cfg in configs.iter().filter(|cfg| cfg.frame.enable) {
        let point_ids = cfg
            .signals
            .iter()
            .map(|signal| match signal {
                CanSignal::Normal(signal) => signal.id,
                CanSignal::Ext(signal) => signal.id,
            })
            .collect();
        map.insert(
            cfg.frame.frame_id,
            FrameBinding {
                frame: cfg.frame,
                point_ids,
            },
        );
    }
    map
}

fn preload_frame_payloads(
    center: &dyn PointCenter,
    payloads: &mut BTreeMap<u32, FramePayload>,
    binding: &FrameBinding,
    point_map: &HashMap<PointId, CanPointConfig>,
    dev_id: &str,
) {
    for point_id in &binding.point_ids {
        let Some(entry) = center.read(dev_id, *point_id) else {
            continue;
        };
        let Some(point_cfg) = point_map.get(point_id) else {
            continue;
        };
        encode_entry(payloads, point_cfg, &entry.value, dev_id);
    }
}

fn encode_entry(
    payloads: &mut BTreeMap<u32, FramePayload>,
    point_cfg: &CanPointConfig,
    value: &Val,
    dev_id: &str,
) {
    match point_cfg.signal {
        CanSignal::Normal(signal) => {
            let Some(raw) = encode_value(
                value,
                signal.bit_len,
                signal.data_type,
                signal.scale,
                signal.offset,
                signal.name,
                dev_id,
            ) else {
                return;
            };
            let payload = payloads
                .entry(point_cfg.frame.frame_id)
                .or_insert_with(|| FramePayload::new(point_cfg.frame, point_cfg.frame.frame_id));
            set_raw(
                &mut payload.data,
                signal.start_bit,
                signal.bit_len,
                signal.byte_order,
                raw,
            );
        }
        CanSignal::Ext(signal) => {
            warn!(
                "[{}] 扩展信号暂不支持点位下发, 忽略: {}",
                dev_id, signal.name
            );
        }
    }
}

fn encode_value(
    value: &Val,
    bit_len: u8,
    data_type: CanDataType,
    scale: f64,
    offset: f64,
    signal_name: &str,
    dev_id: &str,
) -> Option<u32> {
    let numeric: f64 = value.try_into().ok()?;
    let raw = if scale.abs() < f64::EPSILON {
        numeric
    } else {
        (numeric - offset) / scale
    };
    let rounded = raw.round();

    match data_type {
        CanDataType::U8 | CanDataType::U16 | CanDataType::U32 => {
            let max = if bit_len >= 32 {
                u32::MAX as f64
            } else {
                ((1u64 << bit_len) - 1) as f64
            };
            if !(0.0..=max).contains(&rounded) {
                warn!(
                    "[{}] 点位值超出无符号范围, 忽略CAN下发: {}",
                    dev_id, signal_name
                );
                return None;
            }
            Some(rounded as u32)
        }
        CanDataType::I16 | CanDataType::I32 => {
            let bits = u32::from(bit_len.clamp(1, 32));
            let min = -(1i64 << (bits - 1));
            let max = (1i64 << (bits - 1)) - 1;
            if !(min as f64..=max as f64).contains(&rounded) {
                warn!(
                    "[{}] 点位值超出有符号范围, 忽略CAN下发: {}",
                    dev_id, signal_name
                );
                return None;
            }
            let signed = rounded as i64;
            let mask = if bits == 32 {
                u32::MAX as u64
            } else {
                (1u64 << bits) - 1
            };
            Some((signed as u64 & mask) as u32)
        }
    }
}

fn set_raw(data: &mut [u8], start_bit: u8, bit_len: u8, byte_order: ByteOrder, raw: u32) {
    match byte_order {
        ByteOrder::Intel => set_intel(data, start_bit, bit_len, raw),
        ByteOrder::Motorola => set_motorola(data, start_bit, bit_len, raw),
    }
}

fn set_intel(data: &mut [u8], start_bit: u8, bit_len: u8, raw: u32) {
    for bit_idx in 0..usize::from(bit_len) {
        let pos = usize::from(start_bit) + bit_idx;
        let Some(byte) = data.get_mut(pos / 8) else {
            return;
        };
        let mask = 1u8 << (pos % 8);
        if (raw >> bit_idx) & 1 == 1 {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
    }
}

fn set_motorola(data: &mut [u8], start_bit: u8, bit_len: u8, raw: u32) {
    let mut pos = i32::from(start_bit);
    for bit_idx in (0..usize::from(bit_len)).rev() {
        let Ok(pos_usize) = usize::try_from(pos) else {
            return;
        };
        let Some(byte) = data.get_mut(pos_usize / 8) else {
            return;
        };
        let mask = 1u8 << (pos_usize % 8);
        if (raw >> bit_idx) & 1 == 1 {
            *byte |= mask;
        } else {
            *byte &= !mask;
        }
        pos = if pos % 8 == 0 { pos + 15 } else { pos - 1 };
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{build_frame_map, build_point_map, set_raw};
    use crate::config::can_conf::{
        ByteOrder, CanConfig, CanDataType, CanFrameConfig, CanSignal, CanSignalConfig, IdType, Rule,
    };
    use crate::dev::can_dev::runner::extract_raw;

    #[test]
    fn disabled_frames_are_excluded_from_downlink_maps() {
        let enabled = CanConfig {
            frame: CanFrameConfig {
                id: 1,
                name: "enabled",
                frame_id: 0x100,
                id_type: IdType::Standard,
                dlc: 8,
                cycle_duration: Duration::from_millis(100),
                timeout_duration: Duration::from_millis(200),
                send: "a",
                receive: "b",
                rule: Rule::Cycle,
                enable: true,
            },
            signals: vec![CanSignal::Normal(CanSignalConfig {
                id: 10,
                name: "enabled_point",
                frame_id: 0x100,
                signal_name: "enabled_point",
                start_bit: 0,
                bit_len: 8,
                byte_order: ByteOrder::Intel,
                data_type: CanDataType::U8,
                scale: 1.0,
                offset: 0.0,
                unit: "",
                invalid_val: None,
                enum_values: "",
            })],
        };
        let disabled = CanConfig {
            frame: CanFrameConfig {
                id: 2,
                name: "disabled",
                frame_id: 0x200,
                id_type: IdType::Standard,
                dlc: 8,
                cycle_duration: Duration::from_millis(100),
                timeout_duration: Duration::from_millis(200),
                send: "a",
                receive: "b",
                rule: Rule::Cycle,
                enable: false,
            },
            signals: vec![CanSignal::Normal(CanSignalConfig {
                id: 20,
                name: "disabled_point",
                frame_id: 0x200,
                signal_name: "disabled_point",
                start_bit: 0,
                bit_len: 8,
                byte_order: ByteOrder::Intel,
                data_type: CanDataType::U8,
                scale: 1.0,
                offset: 0.0,
                unit: "",
                invalid_val: None,
                enum_values: "",
            })],
        };

        let point_map = build_point_map(&[enabled.clone(), disabled.clone()]);
        let frame_map = build_frame_map(&[enabled, disabled]);

        assert!(point_map.contains_key(&10));
        assert!(!point_map.contains_key(&20));
        assert!(frame_map.contains_key(&0x100));
        assert!(!frame_map.contains_key(&0x200));
    }

    #[test]
    fn intel_bit_packing_round_trips() {
        let mut data = [0u8; 8];

        set_raw(&mut data, 3, 12, ByteOrder::Intel, 0xABC);

        assert_eq!(extract_raw(&data, 3, 12, ByteOrder::Intel), Some(0xABC));
    }

    #[test]
    fn motorola_bit_packing_round_trips() {
        let mut data = [0u8; 8];

        set_raw(&mut data, 15, 12, ByteOrder::Motorola, 0xABC);

        assert_eq!(extract_raw(&data, 15, 12, ByteOrder::Motorola), Some(0xABC));
    }
}
