use std::collections::HashMap;

use anyhow::{Context, anyhow};
use collector_iec60870::{Client, DoublePointValue, StepCommandValue};

use crate::config::iec104_conf::{Iec104Config, PointKind};
use crate::core::point::{PointId, PointRef, Val};

/// 控制点索引，支持按 ID / Key / Name 解析下发目标。
pub(super) struct ControlIndex {
    by_id: HashMap<PointId, Iec104Config>,
    by_key: HashMap<&'static str, PointId>,
    by_name: HashMap<&'static str, PointId>,
}

impl ControlIndex {
    pub(super) fn build(configs: &[Iec104Config]) -> Self {
        let mut index = Self {
            by_id: HashMap::new(),
            by_key: HashMap::new(),
            by_name: HashMap::new(),
        };
        for cfg in configs.iter().filter(|c| c.enable && !c.kind.is_monitor()) {
            index.by_id.insert(cfg.id, *cfg);
            index.by_key.insert(cfg.key, cfg.id);
            if !cfg.name.is_empty() {
                index.by_name.insert(cfg.name, cfg.id);
            }
        }
        index
    }

    pub(super) fn resolve(&self, point: &PointRef) -> Option<&Iec104Config> {
        let id = match point {
            PointRef::Id(id) => *id,
            PointRef::Key(key) => *self.by_key.get(key.as_str())?,
            PointRef::Name(name) => *self.by_name.get(name.as_str())?,
        };
        self.by_id.get(&id)
    }
}

/// 按点表声明的命令类型，把下发值编码为对应的控制 ASDU 并发送（直接执行，不做选择预置）。
pub(super) fn send_command(
    client: &mut Client,
    cfg: &Iec104Config,
    value: &Val,
) -> anyhow::Result<()> {
    let (ca, ioa) = (cfg.ca, cfg.ioa);
    // 设点值：下发工程值 -> 原始值
    let raw = |value: &Val| -> anyhow::Result<f64> {
        let v = value.as_f64().map_err(|_| anyhow!("不支持的下发值类型"))?;
        Ok((v - cfg.offset) / cfg.scale)
    };
    match cfg.kind {
        PointKind::Yk => {
            let on = value.as_bool().map_err(|_| anyhow!("不支持的下发值类型"))?;
            client.send_single_command(ca, ioa, on, false, 0)
        }
        PointKind::YkDouble => {
            let v = match value.as_u32().map_err(|_| anyhow!("不支持的下发值类型"))? {
                1 => DoublePointValue::Off,
                2 => DoublePointValue::On,
                other => return Err(anyhow!("双点遥控值只能是1(分)或2(合): {other}")),
            };
            client.send_double_command(ca, ioa, v, false, 0)
        }
        PointKind::YtShort => {
            client.send_setpoint_command_short(ca, ioa, raw(value)? as f32, false, 0)
        }
        PointKind::YtScaled => {
            let v = raw(value)?.round();
            if !(f64::from(i16::MIN)..=f64::from(i16::MAX)).contains(&v) {
                return Err(anyhow!("标度化值超出范围: {v}"));
            }
            client.send_setpoint_command_scaled(ca, ioa, v as i32, false, 0)
        }
        PointKind::YtNormalized => {
            let v = raw(value)?;
            if !(-1.0..=1.0).contains(&v) {
                return Err(anyhow!("归一化值需在-1~1之间: {v}"));
            }
            client.send_setpoint_command_normalized(ca, ioa, v as f32, false, 0)
        }
        PointKind::YtStep => {
            let v = match value.as_u32().map_err(|_| anyhow!("不支持的下发值类型"))? {
                1 => StepCommandValue::Lower,
                2 => StepCommandValue::Higher,
                other => return Err(anyhow!("步位置命令值只能是1(降)或2(升): {other}")),
            };
            client.send_step_command(ca, ioa, v, false, 0)
        }
        PointKind::Yx | PointKind::Yc => return Err(anyhow!("监视点不支持下发")),
    }
    .with_context(|| format!("CA={ca} IOA={ioa}"))
}
