use std::collections::BTreeMap;

use tokio_modbus::client::{Context, Reader};

use crate::{
    config::modbus_conf::{ByteOrder, ModbusConfig, ModbusDataType, RegisterType},
    core::point::Val,
    dev::modbus_dev::ModbusDevError,
};

#[derive(Debug)]
pub(super) struct Blocks<'a>(pub(super) Vec<Block<'a>>);

#[derive(Debug, thiserror::Error)]
pub enum BuildBlocksError {
    #[error(
        "overlap detected: register_type={register_type:?}, block=[{block_start}..{block_end}), next_start={next_start}"
    )]
    Overlap {
        register_type: RegisterType,
        block_start: u16,
        block_end: u16, // end_excl
        next_start: u16,
    },
}

impl<'a> TryFrom<Vec<&'a ModbusConfig>> for Blocks<'a> {
    type Error = BuildBlocksError;

    fn try_from(value: Vec<&'a ModbusConfig>) -> Result<Self, Self::Error> {
        // 1) 按 RegisterType 分组
        let mut groups: BTreeMap<RegisterType, Vec<&'a ModbusConfig>> = BTreeMap::new();
        for cfg in value {
            groups.entry(cfg.register_type).or_default().push(cfg);
        }

        let mut blocks: Vec<Block<'a>> = Vec::new();

        // 2) 每组：排序 + 连续合并 + 长度限制
        for (rt, mut pts) in groups {
            pts.sort_by_key(|it| it.register_address);

            let max_len: u16 = match rt {
                RegisterType::Coils | RegisterType::DiscreteInputs => 2000,
                RegisterType::HoldingRegisters | RegisterType::InputRegisters => 120,
            };

            let mut i: usize = 0;
            while i < pts.len() {
                let first = pts[i];

                let start = first.register_address;
                let first_w = first.data_type.quantity();

                let mut end_excl = start + first_w;

                let mut regions: Vec<Region<'a>> = Vec::new();
                regions.push(Region {
                    cfg: first,
                    offset: 0,
                    width: first_w,
                });

                i += 1;

                while i < pts.len() {
                    let next = pts[i];
                    let next_start = next.register_address;

                    // 更直观的三分支：连续 / gap / overlap
                    if next_start == end_excl {
                        let next_w = next.data_type.quantity();

                        let cur_len = end_excl - start;
                        if cur_len.saturating_add(next_w) > max_len {
                            break; // 连续但超长 -> 切块
                        }

                        regions.push(Region {
                            cfg: next,
                            offset: next_start - start,
                            width: next_w,
                        });

                        end_excl += next_w;
                        i += 1;
                    } else if next_start > end_excl {
                        break; // gap -> 切块（严格不允许 gap 合并）
                    } else {
                        // next_start < end_excl -> overlap（共享/冲突地址），点表非法
                        return Err(BuildBlocksError::Overlap {
                            register_type: rt,
                            block_start: start,
                            block_end: end_excl,
                            next_start,
                        });
                    }
                }

                blocks.push(Block {
                    register_type: rt,
                    start,
                    len: end_excl - start,
                    regions,
                });
            }
        }

        Ok(Self(blocks))
    }
}

pub(super) enum BlockRead {
    Coils(Vec<bool>),
    DiscreteInputs(Vec<bool>),
    HoldingRegisters(Vec<u16>),
    InputRegisters(Vec<u16>),
}

impl Blocks<'_> {
    pub(super) async fn request(
        &mut self,
        ctx: &mut Context,
    ) -> Result<Vec<BlockRead>, ModbusDevError> {
        let mut reads = Vec::with_capacity(self.0.len());
        for block in &self.0 {
            match block.register_type {
                RegisterType::Coils => {
                    let data = ctx.read_coils(block.start, block.len).await??;
                    reads.push(BlockRead::Coils(data));
                }
                RegisterType::DiscreteInputs => {
                    let data = ctx.read_discrete_inputs(block.start, block.len).await??;
                    reads.push(BlockRead::DiscreteInputs(data));
                }
                RegisterType::HoldingRegisters => {
                    let data = ctx.read_holding_registers(block.start, block.len).await??;
                    reads.push(BlockRead::HoldingRegisters(data));
                }
                RegisterType::InputRegisters => {
                    let data = ctx.read_input_registers(block.start, block.len).await??;
                    reads.push(BlockRead::InputRegisters(data));
                }
            }
        }
        Ok(reads)
    }

    pub(super) fn parse(&self, reads: &[BlockRead]) -> Vec<(String, Val)> {
        let mut out = Vec::new();
        for (block, read) in self.0.iter().zip(reads.iter()) {
            match (block.register_type, read) {
                (RegisterType::Coils, BlockRead::Coils(data))
                | (RegisterType::DiscreteInputs, BlockRead::DiscreteInputs(data)) => {
                    for region in &block.regions {
                        let idx = region.offset as usize;
                        if idx >= data.len() {
                            continue;
                        }
                        let v = if data[idx] { 1u8 } else { 0u8 };
                        out.push((region.cfg.name.clone(), Val::U8(v)));
                    }
                }
                (RegisterType::HoldingRegisters, BlockRead::HoldingRegisters(data))
                | (RegisterType::InputRegisters, BlockRead::InputRegisters(data)) => {
                    for region in &block.regions {
                        let offset = region.offset as usize;
                        let width = region.width as usize;
                        if offset + width > data.len() {
                            continue;
                        }
                        let slice = &data[offset..offset + width];
                        let val = decode_register_value(region.cfg, slice);
                        out.push((region.cfg.name.clone(), val));
                    }
                }
                _ => {}
            }
        }
        out
    }
}

#[derive(Debug)]
pub(super) struct Block<'a> {
    pub(super) register_type: RegisterType,
    pub(super) start: u16,
    pub(super) len: u16,
    pub(super) regions: Vec<Region<'a>>,
}

#[derive(Debug)]
pub(super) struct Region<'a> {
    pub(super) cfg: &'a ModbusConfig,
    pub(super) offset: u16,
    pub(super) width: u16,
}

fn decode_register_value(cfg: &ModbusConfig, data: &[u16]) -> Val {
    match cfg.data_type {
        ModbusDataType::Bool => {
            let v = if data.first().copied().unwrap_or(0) != 0 {
                1u8
            } else {
                0u8
            };
            Val::U8(v)
        }
        ModbusDataType::U16 => {
            let raw = data.first().copied().unwrap_or(0);
            let v = apply_scale_offset(u16_with_order(raw, cfg.byte_order) as f64, cfg);
            to_val_numeric(v)
        }
        ModbusDataType::I16 => {
            let raw = data.first().copied().unwrap_or(0);
            let v = apply_scale_offset(
                i16::from_ne_bytes(u16_with_order(raw, cfg.byte_order).to_ne_bytes()) as f64,
                cfg,
            );
            to_val_numeric(v)
        }
        ModbusDataType::U32 => {
            let raw = u32_with_order(data, cfg.byte_order);
            let v = apply_scale_offset(raw as f64, cfg);
            to_val_numeric(v)
        }
        ModbusDataType::I32 => {
            let raw = u32_with_order(data, cfg.byte_order) as i32;
            let v = apply_scale_offset(raw as f64, cfg);
            to_val_numeric(v)
        }
    }
}

fn u16_with_order(v: u16, order: Option<ByteOrder>) -> u16 {
    match order {
        Some(ByteOrder::BA) => v.swap_bytes(),
        _ => v,
    }
}

fn u32_with_order(data: &[u16], order: Option<ByteOrder>) -> u32 {
    let w0 = data.first().copied().unwrap_or(0);
    let w1 = data.get(1).copied().unwrap_or(0);
    match order {
        Some(ByteOrder::CDAB) => ((w1 as u32) << 16) | (w0 as u32),
        _ => ((w0 as u32) << 16) | (w1 as u32),
    }
}

fn apply_scale_offset(raw: f64, cfg: &ModbusConfig) -> f64 {
    raw * cfg.scale + cfg.offset
}

fn to_val_numeric(v: f64) -> Val {
    if v.fract().abs() < 1e-6 {
        if v >= 0.0 {
            Val::U32(v as u32)
        } else {
            Val::I32(v as i32)
        }
    } else {
        Val::F32(v as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::modbus_conf::ModbusDataType;

    fn cfg(
        register_type: RegisterType,
        register_address: u16,
        data_type: ModbusDataType,
    ) -> ModbusConfig {
        ModbusConfig {
            id: 1,
            name: "p".to_string(),
            data_type,
            unit: None,
            remarks: None,
            register_address,
            register_type,
            quantity: 1,
            byte_order: None,
            scale: 1.0,
            offset: 0.0,
        }
    }

    #[test]
    fn build_blocks_overlap_returns_error() {
        let a = cfg(RegisterType::HoldingRegisters, 10, ModbusDataType::U32); // width 2: [10,12)
        let b = cfg(RegisterType::HoldingRegisters, 11, ModbusDataType::U16); // overlap
        let err = Blocks::try_from(vec![&a, &b]).unwrap_err();
        match err {
            BuildBlocksError::Overlap {
                register_type,
                block_start,
                block_end,
                next_start,
            } => {
                assert_eq!(register_type, RegisterType::HoldingRegisters);
                assert_eq!(block_start, 10);
                assert_eq!(block_end, 12);
                assert_eq!(next_start, 11);
            }
        }
    }

    #[test]
    fn build_blocks_gap_splits_block() {
        let a = cfg(RegisterType::InputRegisters, 0, ModbusDataType::U16);
        let b = cfg(RegisterType::InputRegisters, 2, ModbusDataType::U16); // gap at 1
        let blocks = Blocks::try_from(vec![&a, &b]).unwrap();
        assert_eq!(blocks.0.len(), 2);
        assert_eq!(blocks.0[0].start, 0);
        assert_eq!(blocks.0[0].len, 1);
        assert_eq!(blocks.0[1].start, 2);
        assert_eq!(blocks.0[1].len, 1);
    }

    #[test]
    fn build_blocks_splits_on_max_len() {
        let mut configs: Vec<ModbusConfig> = Vec::new();
        for addr in 0u16..=120u16 {
            configs.push(cfg(
                RegisterType::HoldingRegisters,
                addr,
                ModbusDataType::U16,
            ));
        }
        let refs: Vec<&ModbusConfig> = configs.iter().collect();
        let blocks = Blocks::try_from(refs).unwrap();
        assert_eq!(blocks.0.len(), 2);
        assert_eq!(blocks.0[0].start, 0);
        assert_eq!(blocks.0[0].len, 120);
        assert_eq!(blocks.0[1].start, 120);
        assert_eq!(blocks.0[1].len, 1);
    }
}
