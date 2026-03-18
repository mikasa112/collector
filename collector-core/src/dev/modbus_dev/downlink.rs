use std::collections::{BTreeMap, HashMap};

use smallvec::Array;
use smallvec::SmallVec;
use tokio_modbus::client::{Context, Writer};
use tracing::warn;

use crate::config::modbus_conf::{
    ByteOrder, ModbusConfig, ModbusConfigs, ModbusDataType, RegisterType,
};
use crate::core::point::{DataPoint, PointId, Val, ValError};

use super::error::ModbusDevError;

pub(super) struct WritePlan {
    coils: Vec<(u16, SmallVec<[bool; 16]>)>,
    holding: Vec<(u16, SmallVec<[u16; 16]>)>,
}

impl WritePlan {
    pub(super) fn build(
        entries: Vec<DataPoint>,
        cfg_map: &HashMap<PointId, ModbusConfig>,
        dev_id: &str,
    ) -> Self {
        let mut coils: BTreeMap<u16, bool> = BTreeMap::new();
        let mut holding: BTreeMap<u16, u16> = BTreeMap::new();

        for entry in entries {
            let Some(cfg) = cfg_map.get(&entry.id) else {
                warn!("[{}] 未找到点位配置, 忽略下发: {}", dev_id, entry.id);
                continue;
            };
            match cfg.register_type {
                RegisterType::Coils => {
                    let v: Result<bool, ValError> = (&entry.value).try_into();
                    let Ok(v) = v else {
                        warn!("[{}] 点位类型不支持下发到线圈: {}", dev_id, cfg.name);
                        continue;
                    };
                    coils.insert(cfg.register_address, v);
                }
                RegisterType::HoldingRegisters => {
                    let Some(values) = encode_registers(cfg, &entry.value, dev_id) else {
                        continue;
                    };
                    for (idx, v) in values.into_iter().enumerate() {
                        let addr = cfg.register_address.saturating_add(idx as u16);
                        holding.insert(addr, v);
                    }
                }
                RegisterType::DiscreteInputs | RegisterType::InputRegisters => {
                    warn!("[{}] 只读寄存器不支持下发: {}", dev_id, cfg.name);
                }
            }
        }

        WritePlan {
            coils: merge_blocks::<[bool; 16]>(coils),
            holding: merge_blocks::<[u16; 16]>(holding),
        }
    }

    pub(super) async fn apply(&self, ctx: &mut Context) -> Result<(), ModbusDevError> {
        for (start, vals) in self.coils.iter() {
            if vals.len() == 1 {
                ctx.write_single_coil(*start, vals[0]).await??;
            } else {
                ctx.write_multiple_coils(*start, vals).await??;
            }
        }
        for (start, vals) in self.holding.iter() {
            if vals.len() == 1 {
                ctx.write_single_register(*start, vals[0]).await??;
            } else {
                ctx.write_multiple_registers(*start, vals).await??;
            }
        }
        Ok(())
    }
}

/// 构建以点位ID作为表的配置
/// # 输入
/// - `configs`: MODBUS设备的点位配置列表
/// # 输出
/// - `HashMap<PointId, ModbusConfig>`: 以点位ID作为表的配置映射
pub(super) fn build_cfg_map(configs: &ModbusConfigs) -> HashMap<PointId, ModbusConfig> {
    let mut out = HashMap::new();
    for cfg in configs {
        out.insert(cfg.id as u32, *cfg);
    }
    out
}

fn merge_blocks<A>(map: BTreeMap<u16, A::Item>) -> Vec<(u16, SmallVec<A>)>
where
    A: Array,
    A::Item: Copy,
{
    let mut out: Vec<(u16, SmallVec<A>)> = Vec::new();
    let mut cur_start: Option<u16> = None;
    let mut cur_vals: SmallVec<A> = SmallVec::new();
    let mut last_addr: Option<u16> = None;

    for (addr, val) in map {
        match (cur_start, last_addr) {
            (None, _) => {
                cur_start = Some(addr);
                cur_vals.push(val);
            }
            (Some(_), Some(last)) if addr == last.saturating_add(1) => {
                cur_vals.push(val);
                last_addr = Some(addr);
                continue;
            }
            (Some(start), _) => {
                out.push((start, std::mem::take(&mut cur_vals)));
                cur_start = Some(addr);
                cur_vals.push(val);
            }
        }
        last_addr = Some(addr);
    }

    if let Some(start) = cur_start {
        out.push((start, cur_vals));
    }

    out
}

fn encode_registers(cfg: &ModbusConfig, value: &Val, dev_id: &str) -> Option<SmallVec<[u16; 2]>> {
    match cfg.data_type {
        ModbusDataType::Bool => {
            let v = value.try_into().ok()?;
            let mut out = SmallVec::<[u16; 2]>::new();
            out.push(if v { 1u16 } else { 0u16 });
            Some(out)
        }
        ModbusDataType::U16 => encode_single_register(cfg, value, dev_id, |raw, dev_id, name| {
            to_u16(raw, dev_id, name)
        }),
        ModbusDataType::I16 => encode_single_register(cfg, value, dev_id, |raw, dev_id, name| {
            to_i16(raw, dev_id, name).map(|v| v as u16)
        }),
        ModbusDataType::U32 => encode_double_register(cfg, value, dev_id, |raw, dev_id, name| {
            to_u32(raw, dev_id, name)
        }),
        ModbusDataType::I32 => encode_double_register(cfg, value, dev_id, |raw, dev_id, name| {
            to_i32(raw, dev_id, name).map(|v| v as u32)
        }),
    }
}

fn encode_single_register(
    cfg: &ModbusConfig,
    value: &Val,
    dev_id: &str,
    convert: impl FnOnce(f64, &str, &str) -> Option<u16>,
) -> Option<SmallVec<[u16; 2]>> {
    let raw = scale_to_raw(cfg, value, dev_id)?;
    let v = convert(raw, dev_id, cfg.name)?;
    let mut out = SmallVec::<[u16; 2]>::new();
    out.push(
        cfg.byte_order
            .map_or(ByteOrder::AB.assemble_u16(v), |it| it.assemble_u16(v)),
    );
    Some(out)
}

fn encode_double_register(
    cfg: &ModbusConfig,
    value: &Val,
    dev_id: &str,
    convert: impl FnOnce(f64, &str, &str) -> Option<u32>,
) -> Option<SmallVec<[u16; 2]>> {
    let raw = scale_to_raw(cfg, value, dev_id)?;
    let v = convert(raw, dev_id, cfg.name)?;
    let arr = cfg
        .byte_order
        .map_or(ByteOrder::ABCD.assemble_u32(v), |it| it.assemble_u32(v));
    let mut out = SmallVec::<[u16; 2]>::new();
    out.extend_from_slice(&arr);
    Some(out)
}

fn scale_to_raw(cfg: &ModbusConfig, value: &Val, dev_id: &str) -> Option<f64> {
    let v: f64 = value.try_into().ok()?;
    if cfg.scale.abs() < 1e-12 {
        warn!("[{}] 点位缩放为0, 忽略下发: {}", dev_id, cfg.name);
        return None;
    }
    Some((v - cfg.offset) / cfg.scale)
}

fn to_u16(v: f64, dev_id: &str, name: &str) -> Option<u16> {
    let r = v.round();
    if !(0.0..=u16::MAX as f64).contains(&r) {
        warn!("[{}] 点位值超出U16范围, 忽略下发: {}", dev_id, name);
        return None;
    }
    Some(r as u16)
}

fn to_i16(v: f64, dev_id: &str, name: &str) -> Option<i16> {
    let r = v.round();
    if !(i16::MIN as f64..=i16::MAX as f64).contains(&r) {
        warn!("[{}] 点位值超出I16范围, 忽略下发: {}", dev_id, name);
        return None;
    }
    Some(r as i16)
}

fn to_u32(v: f64, dev_id: &str, name: &str) -> Option<u32> {
    let r = v.round();
    if !(0.0..=u32::MAX as f64).contains(&r) {
        warn!("[{}] 点位值超出U32范围, 忽略下发: {}", dev_id, name);
        return None;
    }
    Some(r as u32)
}

fn to_i32(v: f64, dev_id: &str, name: &str) -> Option<i32> {
    let r = v.round();
    if !(i32::MIN as f64..=i32::MAX as f64).contains(&r) {
        warn!("[{}] 点位值超出I32范围, 忽略下发: {}", dev_id, name);
        return None;
    }
    Some(r as i32)
}
