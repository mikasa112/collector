//! 北向 IEC 104 子站：把 DataCenter 里的数据（含 `emu` 等引擎数据）转发给上级主站，
//! 并把上级的遥控、遥调命令下发回 DataCenter。
//!
//! - 上行：订阅点表里涉及的每个设备，变化（超出死区）时以 COT=突发 上送；设备通讯故障时品质置无效；
//! - 下行：命令按 (CA, IOA) 查控制点，支持选择/执行两步（见 [`CommandCtl`]）；
//! - 总召唤由 [`Slave`] 内部按 CA 响应，尚未收到数据的点以"无效"品质上送。

use std::{
    collections::HashMap,
    path::Path,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use collector_iec60870::{PointValue, ReceivedCommand, Slave, SlaveOptions, SlavePoint, quality};
use parking_lot::Mutex;
use tokio::runtime::Handle;

use crate::{
    center::data_center,
    config::{
        iec104_conf::COMM_STATUS_ID,
        north_iec104_conf::{
            NorthIec104Config, NorthIec104Configs, NorthIec104ConfigsError, build_configs,
        },
        program_conf::NorthIec104Program,
    },
    core::point::{DownDataPoint, PointId},
    shutdown::ShutdownManager,
};

/// 下发命令到 DataCenter 的最长等待时间，超时回否定确认。
const DISPATCH_TIMEOUT: Duration = Duration::from_secs(3);

#[cfg(test)]
mod tests;

#[derive(thiserror::Error, Debug)]
pub enum Iec104ServerError {
    #[error("{0}")]
    OpenConfigsError(#[from] NorthIec104ConfigsError),
    #[error("创建IEC104子站失败: {0}")]
    CreateSlave(String),
}

pub struct Iec104Server {
    configs: Arc<NorthIec104Configs>,
    options: SlaveOptions,
    select_timeout: Duration,
    require_select: bool,
}

impl Iec104Server {
    pub fn new<P: AsRef<Path>>(
        path: P,
        program: &NorthIec104Program,
    ) -> Result<Self, Iec104ServerError> {
        let configs = build_configs(path)?;
        let mut options = SlaveOptions::new(program.port);
        options.local_address = Some(program.host.clone());
        options.max_connections = program.max_connections;
        options.multi_master = program.multi_master;
        Ok(Self {
            configs: Arc::new(configs),
            options,
            select_timeout: Duration::from_millis(program.select_timeout_ms),
            require_select: program.require_select,
        })
    }

    pub async fn start(self, shutdown: ShutdownManager) {
        self.start_inner(shutdown, true).await;
    }

    /// `check_emu_gate` 为 false 时跳过 EMU 控制源检查（仅供测试，没有 EMU 运行时）。
    async fn start_inner(self, shutdown: ShutdownManager, check_emu_gate: bool) {
        let initial: Vec<SlavePoint> = self
            .configs
            .iter()
            .filter_map(|cfg| {
                let mut p = SlavePoint::new(cfg.ca, cfg.ioa, cfg.default_value()?);
                p.quality = quality::INVALID;
                Some(p)
            })
            .collect();
        let addr = format!(
            "{}:{}",
            self.options.local_address.as_deref().unwrap_or("0.0.0.0"),
            self.options.port
        );
        let mut slave = match Slave::with_options(self.options.clone(), initial) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!(
                    "[北向IEC104] {}",
                    Iec104ServerError::CreateSlave(e.to_string())
                );
                return;
            }
        };
        let ctl = Arc::new(CommandCtl::new(
            self.configs.clone(),
            self.select_timeout,
            self.require_select,
            check_emu_gate,
            Handle::current(),
        ));
        slave.set_command_handler(move |cmd| ctl.handle(cmd));
        slave.start();
        tracing::info!(
            "[北向IEC104] 监听 {}, 监视点 {}, 控制点 {}",
            addr,
            self.configs.iter().filter(|c| c.kind.is_monitor()).count(),
            self.configs.iter().filter(|c| !c.kind.is_monitor()).count()
        );
        let slave = Arc::new(slave);

        // 按设备分组：dev -> point_id -> 配置下标
        let mut by_dev: HashMap<String, HashMap<PointId, Vec<usize>>> = HashMap::new();
        for (i, cfg) in self.configs.iter().enumerate() {
            if cfg.kind.is_monitor() {
                by_dev
                    .entry(cfg.source.clone())
                    .or_default()
                    .entry(cfg.point_id)
                    .or_default()
                    .push(i);
            }
        }
        let tasks: Vec<_> = by_dev
            .into_iter()
            .map(|(dev, index)| {
                tokio::spawn(forward_dev(
                    dev,
                    index,
                    self.configs.clone(),
                    slave.clone(),
                    shutdown.clone(),
                ))
            })
            .collect();

        shutdown.wait_for_shutdown().await;
        for t in tasks {
            let _ = t.await;
        }
        // Slave 析构会停止监听并等待内部线程退出，放到阻塞线程里做
        let _ = tokio::task::spawn_blocking(move || drop(slave)).await;
    }
}

/// 最近一次上送的内容，用于变化检测与死区判断。
struct Sent {
    value: PointValue,
    quality: u8,
    /// 上送时的工程值，死区按它比较
    eng: Option<f64>,
}

impl Sent {
    /// 是否需要上送新的 (值, 品质)。
    fn differs(
        &self,
        cfg: &NorthIec104Config,
        value: &PointValue,
        q: u8,
        eng: Option<f64>,
    ) -> bool {
        if self.quality != q {
            return true;
        }
        if cfg.deadband > 0.0
            && let (Some(old), Some(new)) = (self.eng, eng)
        {
            return (new - old).abs() >= cfg.deadband;
        }
        self.value != *value
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// 订阅一个设备，把其中被点表引用的点转发给 `slave`。
async fn forward_dev(
    dev: String,
    index: HashMap<PointId, Vec<usize>>,
    configs: Arc<NorthIec104Configs>,
    slave: Arc<Slave>,
    shutdown: ShutdownManager,
) {
    // 设备可能晚于本服务注册到 DataCenter，期间每秒重试
    let mut rx = loop {
        if let Some(rx) = data_center().subscribe(&dev) {
            break rx;
        }
        tokio::select! {
            _ = shutdown.wait_for_shutdown() => return,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
    };
    let mut sent: HashMap<usize, Sent> = HashMap::new();
    loop {
        let snapshot = rx.borrow_and_update().clone();
        // 通讯状态点：非 0 表示设备通讯故障，其下所有点品质置无效
        let faulted = snapshot
            .iter()
            .find(|p| p.id == COMM_STATUS_ID)
            .and_then(|p| p.value.as_bool().ok())
            .unwrap_or(false);
        let q = if faulted {
            quality::INVALID
        } else {
            quality::GOOD
        };
        let mut changed = Vec::new();
        for point in snapshot.iter() {
            let Some(indices) = index.get(&point.id) else {
                continue;
            };
            for &i in indices {
                let cfg = &configs[i];
                let Some(value) = cfg.encode(&point.value) else {
                    continue;
                };
                let eng = point.value.as_f64().ok();
                if sent
                    .get(&i)
                    .is_some_and(|s| !s.differs(cfg, &value, q, eng))
                {
                    continue;
                }
                sent.insert(
                    i,
                    Sent {
                        value,
                        quality: q,
                        eng,
                    },
                );
                changed.push(SlavePoint {
                    ca: cfg.ca,
                    ioa: cfg.ioa,
                    value,
                    quality: q,
                    timestamp_ms: cfg.timestamp.then(now_ms),
                });
            }
        }
        if !changed.is_empty() {
            slave.update_many(&changed);
        }
        tokio::select! {
            _ = shutdown.wait_for_shutdown() => return,
            changed = rx.changed() => if changed.is_err() { return },
        }
    }
}

/// 命令处理：查控制点、两步命令的选择状态、下发到 DataCenter。
struct CommandCtl {
    configs: Arc<NorthIec104Configs>,
    /// (CA, IOA) -> 控制点配置下标
    index: HashMap<(i32, i32), usize>,
    /// 已选择、待执行的命令 (CA, IOA) -> 选择时刻
    selected: Mutex<HashMap<(i32, i32), Instant>>,
    select_timeout: Duration,
    require_select: bool,
    /// 是否检查 EMU 控制源；单测里没有 EMU 运行时，关闭
    check_emu_gate: bool,
    handle: Handle,
}

impl CommandCtl {
    fn new(
        configs: Arc<NorthIec104Configs>,
        select_timeout: Duration,
        require_select: bool,
        check_emu_gate: bool,
        handle: Handle,
    ) -> Self {
        let index = configs
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.kind.is_monitor())
            .map(|(i, c)| ((c.ca, c.ioa), i))
            .collect();
        Self {
            configs,
            index,
            selected: Mutex::new(HashMap::new()),
            select_timeout,
            require_select,
            check_emu_gate,
            handle,
        }
    }

    /// 返回 `true` 回肯定确认，`false` 回否定确认。
    /// 在库的连接线程里调用，下发用 `block_on` 同步等待结果。
    fn handle(&self, cmd: &ReceivedCommand) -> bool {
        let key = (cmd.ca, cmd.ioa);
        let Some(&i) = self.index.get(&key) else {
            tracing::warn!("[北向IEC104] 未配置的控制点 CA={} IOA={}", cmd.ca, cmd.ioa);
            return false;
        };
        let cfg = &self.configs[i];
        let Some(value) = cfg.decode_command(&cmd.value) else {
            tracing::warn!(
                "[北向IEC104] 命令不合法 {} CA={} IOA={} {:?}",
                cfg.name,
                cmd.ca,
                cmd.ioa,
                cmd.value
            );
            return false;
        };
        let down = DownDataPoint::by_id(cfg.point_id, value);

        if cmd.select {
            // 预置前先确认下发条件，避免选择成功、执行才失败
            if !self
                .handle
                .block_on(can_dispatch(&cfg.source, &down, self.check_emu_gate))
            {
                return false;
            }
            let mut selected = self.selected.lock();
            let timeout = self.select_timeout;
            selected.retain(|_, at| at.elapsed() < timeout);
            selected.insert(key, Instant::now());
            tracing::info!("[北向IEC104] 选择 {}", cfg.name);
            return true;
        }

        let preselected = self
            .selected
            .lock()
            .remove(&key)
            .is_some_and(|at| at.elapsed() < self.select_timeout);
        if self.require_select && !preselected {
            tracing::warn!("[北向IEC104] 执行前未选择或选择已超时 {}", cfg.name);
            return false;
        }
        tracing::info!("[北向IEC104] ↓ {}", cfg.name);
        let source = cfg.source.clone();
        let check_emu_gate = self.check_emu_gate;
        self.handle.block_on(async move {
            tokio::time::timeout(DISPATCH_TIMEOUT, async {
                if !can_dispatch(&source, &down, check_emu_gate).await {
                    return false;
                }
                match data_center().dispatch(&source, vec![down]).await {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::error!("[北向IEC104] 下发失败: {}", e);
                        false
                    }
                }
            })
            .await
            .unwrap_or_else(|_| {
                tracing::error!("[北向IEC104] 下发超时");
                false
            })
        })
    }
}

/// EMU 控制源为本地时不允许北向下发（未启用 EMU 时默认放行），运行模式点位除外；
/// 目标设备没有下行通道时也无法下发。
async fn can_dispatch(source: &str, down: &DownDataPoint, check_emu_gate: bool) -> bool {
    if !data_center().has_downlink(source) {
        tracing::warn!("[北向IEC104] 设备 {} 没有下行通道", source);
        return false;
    }
    let blocked = check_emu_gate
        && match crate::runtime::core::get_runtime().await {
            Ok(runtime) => !runtime.emu_runtime.allow_remote_dispatch(),
            Err(_) => false,
        };
    if blocked && !crate::runtime::emu::is_run_mode_down(source, down) {
        tracing::warn!("[北向IEC104] EMU控制源为本地, 已拒绝北向下发");
        return false;
    }
    true
}
