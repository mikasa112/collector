use std::collections::{BTreeMap, HashMap};

use tokio_modbus::client::{Context, Writer};
use tracing::warn;

use crate::center::data_center::Entry;
use crate::config::modbus_conf::{
    ByteOrder, ModbusConfig, ModbusConfigs, ModbusDataType, RegisterType,
};
use crate::core::point::Val;

use super::error::ModbusDevError;

pub(super) struct WritePlan {
    coils: Vec<(u16, Vec<bool>)>,
    holding: Vec<(u16, Vec<u16>)>,
}

impl WritePlan {
    pub(super) fn build(
        entries: Vec<Entry>,
        cfg_map: &HashMap<String, ModbusConfig>,
        dev_id: &str,
    ) -> Self {
        let mut coils: BTreeMap<u16, bool> = BTreeMap::new();
        let mut holding: BTreeMap<u16, u16> = BTreeMap::new();

        for entry in entries {
            let Some(cfg) = cfg_map.get(&entry.key) else {
                warn!("[{}] 未找到点位配置, 忽略下发: {}", dev_id, entry.key);
                continue;
            };
            match cfg.register_type {
                RegisterType::Coils => {
                    let Some(v) = val_to_bool(entry.value) else {
                        warn!("[{}] 点位类型不支持下发到线圈: {}", dev_id, cfg.name);
                        continue;
                    };
                    coils.insert(cfg.register_address, v);
                }
                RegisterType::HoldingRegisters => {
                    let Some(values) = encode_registers(cfg, entry.value, dev_id) else {
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
            coils: merge_bool_blocks(coils),
            holding: merge_u16_blocks(holding),
        }
    }
}

pub(super) fn build_cfg_map(configs: &ModbusConfigs) -> HashMap<String, ModbusConfig> {
    let mut out = HashMap::new();
    for cfg in configs {
        out.insert(cfg.name.clone(), cfg.clone());
    }
    out
}

fn merge_bool_blocks(map: BTreeMap<u16, bool>) -> Vec<(u16, Vec<bool>)> {
    let mut out: Vec<(u16, Vec<bool>)> = Vec::new();
    let mut cur_start: Option<u16> = None;
    let mut cur_vals: Vec<bool> = Vec::new();
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

fn merge_u16_blocks(map: BTreeMap<u16, u16>) -> Vec<(u16, Vec<u16>)> {
    let mut out: Vec<(u16, Vec<u16>)> = Vec::new();
    let mut cur_start: Option<u16> = None;
    let mut cur_vals: Vec<u16> = Vec::new();
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

pub(super) async fn apply_write_plan(
    ctx: &mut Context,
    plan: WritePlan,
) -> Result<(), ModbusDevError> {
    for (start, vals) in plan.coils {
        if vals.len() == 1 {
            ctx.write_single_coil(start, vals[0]).await??;
        } else {
            ctx.write_multiple_coils(start, &vals).await??;
        }
    }
    for (start, vals) in plan.holding {
        if vals.len() == 1 {
            ctx.write_single_register(start, vals[0]).await??;
        } else {
            ctx.write_multiple_registers(start, &vals).await??;
        }
    }
    Ok(())
}

fn val_to_bool(v: Val) -> Option<bool> {
    match v {
        Val::U8(v) => Some(v != 0),
        Val::I8(v) => Some(v != 0),
        Val::I16(v) => Some(v != 0),
        Val::I32(v) => Some(v != 0),
        Val::U16(v) => Some(v != 0),
        Val::U32(v) => Some(v != 0),
        Val::F32(v) => Some(v.abs() > f32::EPSILON),
    }
}

fn val_to_f64(v: Val) -> Option<f64> {
    match v {
        Val::U8(v) => Some(v as f64),
        Val::I8(v) => Some(v as f64),
        Val::I16(v) => Some(v as f64),
        Val::I32(v) => Some(v as f64),
        Val::U16(v) => Some(v as f64),
        Val::U32(v) => Some(v as f64),
        Val::F32(v) => Some(v as f64),
    }
}

fn encode_registers(cfg: &ModbusConfig, value: Val, dev_id: &str) -> Option<Vec<u16>> {
    match cfg.data_type {
        ModbusDataType::Bool => {
            let v = val_to_bool(value)?;
            Some(vec![if v { 1u16 } else { 0u16 }])
        }
        ModbusDataType::U16 => {
            let raw = scale_to_raw(cfg, value, dev_id)?;
            let v = to_u16(raw, dev_id, &cfg.name)?;
            Some(vec![u16_with_order(v, cfg.byte_order)])
        }
        ModbusDataType::I16 => {
            let raw = scale_to_raw(cfg, value, dev_id)?;
            let v = to_i16(raw, dev_id, &cfg.name)? as u16;
            Some(vec![u16_with_order(v, cfg.byte_order)])
        }
        ModbusDataType::U32 => {
            let raw = scale_to_raw(cfg, value, dev_id)?;
            let v = to_u32(raw, dev_id, &cfg.name)?;
            Some(encode_u32(v, cfg.byte_order).to_vec())
        }
        ModbusDataType::I32 => {
            let raw = scale_to_raw(cfg, value, dev_id)?;
            let v = to_i32(raw, dev_id, &cfg.name)? as u32;
            Some(encode_u32(v, cfg.byte_order).to_vec())
        }
    }
}

fn scale_to_raw(cfg: &ModbusConfig, value: Val, dev_id: &str) -> Option<f64> {
    let v = val_to_f64(value)?;
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

fn u16_with_order(v: u16, order: Option<ByteOrder>) -> u16 {
    match order {
        Some(ByteOrder::BA) => v.swap_bytes(),
        _ => v,
    }
}

fn encode_u32(raw: u32, order: Option<ByteOrder>) -> [u16; 2] {
    let w0 = (raw >> 16) as u16;
    let w1 = (raw & 0xFFFF) as u16;
    match order {
        Some(ByteOrder::CDAB) => [w1, w0],
        _ => [w0, w1],
    }
}
