use std::collections::BTreeMap;

use tokio_modbus::client::{Context, Reader};

use crate::{
    config::modbus_conf::{ByteOrder, ModbusConfig, ModbusDataType, RegisterType},
    core::point::{DataPoint, Val},
    dev::modbus_dev::ModbusDevError,
};

#[derive(Debug)]
pub(super) struct Blocks {
    pub(super) blocks: Vec<Block>,
    logical_regions: Vec<LogicalRegion>,
}

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

impl TryFrom<Vec<ModbusConfig>> for Blocks {
    type Error = BuildBlocksError;

    fn try_from(value: Vec<ModbusConfig>) -> Result<Self, Self::Error> {
        // 1) 按 RegisterType 分组
        let mut groups: BTreeMap<RegisterType, Vec<ModbusConfig>> = BTreeMap::new();
        for cfg in value {
            groups.entry(cfg.register_type).or_default().push(cfg);
        }

        let mut blocks: Vec<Block> = Vec::new();
        let mut logical_regions: Vec<LogicalRegion> = Vec::new();

        // 2) 每组：排序 + 连续合并 + 长度限制
        for (rt, mut pts) in groups {
            pts.sort_by_key(|it| it.register_address);
            //不同的寄存器允许连续读取的长度不同
            let max_len: u16 = match rt {
                RegisterType::Coils | RegisterType::DiscreteInputs => 2000,
                RegisterType::HoldingRegisters | RegisterType::InputRegisters => 120,
            };

            let mut active_range: Option<(u16, u16)> = None;
            let mut current_block: Option<Block> = None;

            for cfg in pts {
                let cfg_start = cfg.register_address;
                let cfg_end = cfg.register_address.saturating_add(cfg.quantity);

                match active_range {
                    Some((block_start, block_end)) if cfg_start < block_end => {
                        return Err(BuildBlocksError::Overlap {
                            register_type: rt,
                            block_start,
                            block_end,
                            next_start: cfg_start,
                        });
                    }
                    Some((_, block_end)) if cfg_start > block_end => {
                        active_range = Some((cfg_start, cfg_end));
                    }
                    Some((block_start, _)) => {
                        active_range = Some((block_start, cfg_end));
                    }
                    None => {
                        active_range = Some((cfg_start, cfg_end));
                    }
                }

                let region_idx = logical_regions.len();
                logical_regions.push(LogicalRegion { cfg });

                let mut region_offset = 0u16;
                let mut next_addr = cfg.register_address;
                let mut remaining = cfg.quantity;
                while remaining > 0 {
                    let mut appendable = false;
                    if let Some(block) = current_block.as_ref() {
                        appendable = block.register_type == rt
                            && block.start.saturating_add(block.len) == next_addr
                            && block.len < max_len;
                    }

                    if !appendable {
                        if let Some(block) = current_block.take() {
                            blocks.push(block);
                        }
                        current_block = Some(Block {
                            register_type: rt,
                            start: next_addr,
                            len: 0,
                            segments: Vec::new(),
                        });
                    }

                    let block = current_block.as_mut().expect("block just initialized");
                    let capacity = max_len.saturating_sub(block.len);
                    let width = remaining.min(capacity);
                    let block_offset = block.len;
                    block.segments.push(RegionSegment {
                        region_idx,
                        block_offset,
                        region_offset,
                        width,
                    });
                    block.len = block.len.saturating_add(width);
                    remaining = remaining.saturating_sub(width);
                    next_addr = next_addr.saturating_add(width);
                    region_offset = region_offset.saturating_add(width);

                    if block.len >= max_len {
                        let block = current_block.take().expect("block exists");
                        blocks.push(block);
                    }
                }
            }

            if let Some(block) = current_block.take() {
                blocks.push(block);
            }
        }

        Ok(Self {
            blocks,
            logical_regions,
        })
    }
}

pub(super) enum BlockRead {
    Coils(Vec<bool>),
    DiscreteInputs(Vec<bool>),
    HoldingRegisters(Vec<u16>),
    InputRegisters(Vec<u16>),
}

impl Blocks {
    /// 读取四遥的值
    /// # 输入
    pub(super) async fn request(
        &self,
        ctx: &mut Context,
    ) -> Result<Vec<BlockRead>, ModbusDevError> {
        let mut reads = Vec::with_capacity(self.blocks.len());
        for block in &self.blocks {
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

    pub(super) fn parse(&self, reads: &[BlockRead]) -> Vec<DataPoint> {
        let mut reg_values: Vec<Option<CollectState<u16>>> = vec![None; self.logical_regions.len()];
        let mut bit_values: Vec<Option<CollectState<bool>>> =
            vec![None; self.logical_regions.len()];

        for (block, read) in self.blocks.iter().zip(reads.iter()) {
            match (block.register_type, read) {
                (RegisterType::Coils, BlockRead::Coils(data))
                | (RegisterType::DiscreteInputs, BlockRead::DiscreteInputs(data)) => {
                    collect_segments(
                        &self.logical_regions,
                        &block.segments,
                        data,
                        &mut bit_values,
                    );
                }
                (RegisterType::HoldingRegisters, BlockRead::HoldingRegisters(data))
                | (RegisterType::InputRegisters, BlockRead::InputRegisters(data)) => {
                    collect_segments(
                        &self.logical_regions,
                        &block.segments,
                        data,
                        &mut reg_values,
                    );
                }
                _ => {}
            }
        }

        let mut out = Vec::with_capacity(self.logical_regions.len());
        for (idx, region) in self.logical_regions.iter().enumerate() {
            let value = match region.cfg.register_type {
                RegisterType::Coils | RegisterType::DiscreteInputs => bit_values[idx]
                    .as_ref()
                    .filter(|state| state.filled == region.cfg.quantity as usize)
                    .map(|state| decode_bit_value(&region.cfg, &state.values)),
                RegisterType::HoldingRegisters | RegisterType::InputRegisters => reg_values[idx]
                    .as_ref()
                    .filter(|state| state.filled == region.cfg.quantity as usize)
                    .map(|state| decode_register_value(&region.cfg, &state.values)),
            };
            let Some(value) = value else {
                continue;
            };
            out.push(DataPoint {
                id: region.cfg.id as u32,
                name: region.cfg.name,
                value,
            });
        }
        out
    }
}

#[derive(Debug)]
pub(super) struct Block {
    pub(super) register_type: RegisterType,
    pub(super) start: u16,
    pub(super) len: u16,
    pub(super) segments: Vec<RegionSegment>,
}

#[derive(Debug)]
struct LogicalRegion {
    cfg: ModbusConfig,
}

#[derive(Debug)]
pub(super) struct RegionSegment {
    pub(super) region_idx: usize,
    pub(super) block_offset: u16,
    pub(super) region_offset: u16,
    pub(super) width: u16,
}

#[derive(Debug, Clone)]
struct CollectState<T> {
    values: Vec<T>,
    filled: usize,
}

fn collect_segments<T: Copy + Default>(
    logical_regions: &[LogicalRegion],
    segments: &[RegionSegment],
    data: &[T],
    states: &mut [Option<CollectState<T>>],
) {
    for segment in segments {
        let src_offset = segment.block_offset as usize;
        let dst_offset = segment.region_offset as usize;
        let width = segment.width as usize;
        if src_offset + width > data.len() {
            continue;
        }
        let region = &logical_regions[segment.region_idx];
        let state = states[segment.region_idx].get_or_insert_with(|| CollectState {
            values: vec![T::default(); region.cfg.quantity as usize],
            filled: 0,
        });
        state.values[dst_offset..dst_offset + width]
            .copy_from_slice(&data[src_offset..src_offset + width]);
        state.filled += width;
    }
}

fn decode_bit_value(cfg: &ModbusConfig, data: &[bool]) -> Val {
    if cfg.quantity == 1 {
        return Val::U8(if data.first().copied().unwrap_or(false) {
            1
        } else {
            0
        });
    }
    Val::List(
        data.iter()
            .map(|it| Val::U8(if *it { 1 } else { 0 }))
            .collect(),
    )
}

fn decode_scalar(cfg: &ModbusConfig, data: &[u16]) -> Val {
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

fn decode_register_value(cfg: &ModbusConfig, data: &[u16]) -> Val {
    let item_width = cfg.data_type.register_width() as usize;
    if cfg.quantity as usize == item_width {
        return decode_scalar(cfg, data);
    }
    let mut out = Vec::with_capacity(data.len() / item_width);
    for chunk in data.chunks(item_width) {
        if chunk.len() < item_width {
            break;
        }
        out.push(decode_scalar(cfg, chunk));
    }
    Val::List(out)
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
            name: "p",
            data_type,
            unit: None,
            remarks: None,
            register_address,
            register_type,
            quantity: data_type.register_width(),
            byte_order: None,
            scale: 1.0,
            offset: 0.0,
        }
    }

    #[test]
    fn build_blocks_overlap_returns_error() {
        let a = cfg(RegisterType::HoldingRegisters, 10, ModbusDataType::U32); // width 2: [10,12)
        let b = cfg(RegisterType::HoldingRegisters, 11, ModbusDataType::U16); // overlap
        let err = Blocks::try_from(vec![a, b]).unwrap_err();
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
        let blocks = Blocks::try_from(vec![a, b]).unwrap();
        assert_eq!(blocks.blocks.len(), 2);
        assert_eq!(blocks.blocks[0].start, 0);
        assert_eq!(blocks.blocks[0].len, 1);
        assert_eq!(blocks.blocks[1].start, 2);
        assert_eq!(blocks.blocks[1].len, 1);
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
        let blocks = Blocks::try_from(configs).unwrap();
        assert_eq!(blocks.blocks.len(), 2);
        assert_eq!(blocks.blocks[0].start, 0);
        assert_eq!(blocks.blocks[0].len, 120);
        assert_eq!(blocks.blocks[1].start, 120);
        assert_eq!(blocks.blocks[1].len, 1);
    }

    #[test]
    fn decode_register_value_returns_list_for_multi_u16() {
        let mut cfg = cfg(RegisterType::InputRegisters, 0, ModbusDataType::U16);
        cfg.quantity = 3;
        cfg.scale = 0.1;

        let val = decode_register_value(&cfg, &[10, 20, 30]);

        assert_eq!(val, Val::List(vec![Val::U32(1), Val::U32(2), Val::U32(3)]));
    }

    #[test]
    fn build_blocks_splits_single_large_region_across_blocks() {
        let mut point = cfg(RegisterType::InputRegisters, 1000, ModbusDataType::U16);
        point.quantity = 416;
        let blocks = Blocks::try_from(vec![point]).unwrap();

        assert_eq!(blocks.blocks.len(), 4);
        assert_eq!(blocks.blocks[0].start, 1000);
        assert_eq!(blocks.blocks[0].len, 120);
        assert_eq!(blocks.blocks[1].start, 1120);
        assert_eq!(blocks.blocks[1].len, 120);
        assert_eq!(blocks.blocks[2].start, 1240);
        assert_eq!(blocks.blocks[2].len, 120);
        assert_eq!(blocks.blocks[3].start, 1360);
        assert_eq!(blocks.blocks[3].len, 56);
    }

    #[test]
    fn parse_reassembles_large_region_from_multiple_blocks() {
        let mut point = cfg(RegisterType::InputRegisters, 1000, ModbusDataType::U16);
        point.quantity = 121;
        point.scale = 0.1;
        let blocks = Blocks::try_from(vec![point]).unwrap();
        let mut first = vec![0u16; 120];
        first[0] = 10;
        first[119] = 1200;
        let reads = vec![
            BlockRead::InputRegisters(first),
            BlockRead::InputRegisters(vec![1210]),
        ];

        let parsed = blocks.parse(&reads);

        assert_eq!(parsed.len(), 1);
        let Val::List(items) = &parsed[0].value else {
            panic!("expected list");
        };
        assert_eq!(items.len(), 121);
        assert_eq!(items[0], Val::U32(1));
        assert_eq!(items[119], Val::U32(120));
        assert_eq!(items[120], Val::U32(121));
    }

    #[test]
    fn parse_skips_incomplete_large_region() {
        let mut point = cfg(RegisterType::InputRegisters, 1000, ModbusDataType::U16);
        point.quantity = 121;
        let blocks = Blocks::try_from(vec![point]).unwrap();

        let parsed = blocks.parse(&[BlockRead::InputRegisters(vec![1; 120])]);

        assert!(parsed.is_empty());
    }
}
