use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, anyhow};
use collector_iec60870::{Client, DoublePointValue, StepCommandValue};
use tokio::sync::oneshot;

use crate::config::iec104_conf::{Iec104Config, PointKind};
use crate::core::point::{PointId, PointRef, Val};

/// 选择命令发出后等待子站确认的最长时间，超时视为失败且不再发执行。
const SELECT_CONFIRM_TIMEOUT: Duration = Duration::from_secs(3);

/// 单点遥控 C_SC_NA_1 / 双点遥控 C_DC_NA_1 的类型标识。
const TYPE_ID_SINGLE_COMMAND: u32 = 45;
const TYPE_ID_DOUBLE_COMMAND: u32 = 46;
/// 传输原因：激活确认；44~47 为子站对未知类型/原因/公共地址/信息对象地址的否定回应。
const COT_ACTIVATION_CON: u32 = 7;
const COT_UNKNOWN_FIRST: u32 = 44;
const COT_UNKNOWN_LAST: u32 = 47;

/// 正在等待选择确认的遥控点：(CA, IOA) -> 确认结果（true 为肯定确认）。
/// 由发送侧登记，接收线程里的 [`notify_command_confirm`] 兑现。
pub(super) type PendingSelects = Arc<Mutex<HashMap<(i32, i32), oneshot::Sender<bool>>>>;

/// 收到遥控命令的确认类 ASDU 时唤醒对应的选择等待；不是遥控确认或无人等待时返回 `false`。
/// 只有选择阶段会登记等待，执行命令的确认到达时已无人登记，会被忽略。
pub(super) fn notify_command_confirm(
    pending: &PendingSelects,
    type_id: u32,
    cot: u32,
    is_negative: bool,
    ca: i32,
    ioas: impl IntoIterator<Item = i32>,
) -> bool {
    if !matches!(type_id, TYPE_ID_SINGLE_COMMAND | TYPE_ID_DOUBLE_COMMAND) {
        return false;
    }
    let confirmed = if cot == COT_ACTIVATION_CON {
        !is_negative
    } else if (COT_UNKNOWN_FIRST..=COT_UNKNOWN_LAST).contains(&cot) {
        false
    } else {
        return false;
    };
    let mut pending = pending.lock().unwrap_or_else(|e| e.into_inner());
    let mut hit = false;
    for ioa in ioas {
        if let Some(tx) = pending.remove(&(ca, ioa)) {
            let _ = tx.send(confirmed);
            hit = true;
        }
    }
    hit
}

/// 控制点索引，支持按 ID / Key / Name 解析下发目标。
pub(super) struct ControlIndex {
    by_id: HashMap<PointId, Iec104Config>,
    by_key: HashMap<Arc<str>, PointId>,
    by_name: HashMap<Arc<str>, PointId>,
}

impl ControlIndex {
    pub(super) fn build(configs: &[Iec104Config]) -> Self {
        let mut index = Self {
            by_id: HashMap::new(),
            by_key: HashMap::new(),
            by_name: HashMap::new(),
        };
        for cfg in configs.iter().filter(|c| c.enable && !c.kind.is_monitor()) {
            index.by_id.insert(cfg.id, cfg.clone());
            index.by_key.insert(cfg.key.clone(), cfg.id);
            if !cfg.name.is_empty() {
                index.by_name.insert(cfg.name.clone(), cfg.id);
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

/// 下发一个控制点。点表 `select` 为真时走"选择+执行"：先发选择并等待子站肯定确认，
/// 否定确认、超时或发送失败都直接返回错误且不发执行；否则直接执行。
/// 执行命令本身不等待确认，发出即视为成功。
pub(super) async fn dispatch_command(
    client: &mut Client,
    cfg: &Iec104Config,
    value: &Val,
    pending: &PendingSelects,
) -> anyhow::Result<()> {
    if !cfg.select {
        return send_command(client, cfg, value, false);
    }
    let key = (cfg.ca, cfg.ioa);
    let (tx, rx) = oneshot::channel();
    // 先登记再发送，避免确认先于登记到达而丢失
    pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key, tx);
    let result = async {
        send_command(client, cfg, value, true)?;
        match tokio::time::timeout(SELECT_CONFIRM_TIMEOUT, rx).await {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(anyhow!("选择被子站否定确认")),
            Ok(Err(_)) => Err(anyhow!("选择确认等待被中断")),
            Err(_) => Err(anyhow!(
                "选择命令{}秒内未收到确认",
                SELECT_CONFIRM_TIMEOUT.as_secs()
            )),
        }
    }
    .await;
    // 超时、发送失败时清掉登记，不留残余
    pending
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&key);
    result.context("遥控选择阶段失败, 未发执行")?;
    send_command(client, cfg, value, false)
}

/// 按点表声明的命令类型，把下发值编码为对应的控制 ASDU 并发送。
/// `select` 仅对单点/双点遥控有效：`true` 发选择，`false` 发执行；设点等命令恒为直接执行。
fn send_command(
    client: &mut Client,
    cfg: &Iec104Config,
    value: &Val,
    select: bool,
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
            client.send_single_command(ca, ioa, on, select, 0)
        }
        PointKind::YkDouble => {
            let v = match value.as_u32().map_err(|_| anyhow!("不支持的下发值类型"))? {
                1 => DoublePointValue::Off,
                2 => DoublePointValue::On,
                other => return Err(anyhow!("双点遥控值只能是1(分)或2(合): {other}")),
            };
            client.send_double_command(ca, ioa, v, select, 0)
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
