//! 一次连接建立后的"会话": 解析点表里的引用、订阅报告、判断哪些点要靠轮询兜底、安排周期任务。
//!
//! 这里的函数全部是同步阻塞的(MMS 请求), 必须在 `spawn_blocking` 里调用。报告回调则跑在
//! libiec61850 自己的接收线程上, 只做内存里的展开和入库, 不发任何请求。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use collector_iec61850::{
    Connection, DataSetLayout, Error, Fc, ObjectRef, Report, ReportConfig, TRG_DATA_CHANGED,
    TRG_GI, TRG_QUALITY_CHANGED, Value,
};
use tracing::{debug, info, warn};

use crate::center::data_center;
use crate::config::iec61850_conf::{Iec61850Config, Iec61850Configs, SourceKind};
use crate::core::point::PointId;

use super::convert::decode_value;

/// 把读到的叶子值按引用对应到点, 转换后写入数据中心。
pub(super) struct Dispatcher {
    dev_id: String,
    points: Vec<Iec61850Config>,
    /// 已把 LD 解析成设备上真实名字、带 FC 的叶子引用 -> 点
    by_ref: HashMap<ObjectRef, usize>,
    /// 类型不匹配的告警每个点只报一次, 避免报告里每次都刷屏
    warned: Mutex<HashSet<PointId>>,
}

impl Dispatcher {
    fn new(dev_id: &str, resolved: Vec<(Iec61850Config, ObjectRef)>) -> Self {
        let mut points = Vec::with_capacity(resolved.len());
        let mut by_ref = HashMap::with_capacity(resolved.len());
        for (cfg, reference) in resolved {
            by_ref.insert(reference, points.len());
            points.push(cfg);
        }
        Self {
            dev_id: dev_id.to_string(),
            points,
            by_ref,
            warned: Mutex::new(HashSet::new()),
        }
    }

    /// 不在点表里的叶子直接忽略(数据集里常带着点表不关心的成员)。
    pub(super) fn ingest(&self, leaves: impl IntoIterator<Item = (ObjectRef, Value)>) {
        let mut out = Vec::new();
        for (reference, value) in leaves {
            let Some(&idx) = self.by_ref.get(&reference) else {
                continue;
            };
            let cfg = &self.points[idx];
            match decode_value(cfg, &value) {
                Some(val) => out.push(cfg.to_data_point(val)),
                None => {
                    if self.warned.lock().unwrap().insert(cfg.id) {
                        warn!(
                            "[{}] 点 {}({}) 收到的值与点类型 {:?} 不匹配, 已丢弃: {:?}",
                            self.dev_id, cfg.key, reference, cfg.kind, value
                        );
                    }
                }
            }
        }
        if !out.is_empty() {
            data_center().ingest(&self.dev_id, out);
        }
    }
}

/// 没有被任何报告或数据集覆盖、只能逐点读取的监视点。
struct PollPoint {
    reference: ObjectRef,
    dotted: String,
    fc: Fc,
}

struct PolledDataSet {
    name: String,
    layout: Arc<DataSetLayout>,
}

pub(super) enum Job {
    /// 对已使能的 RCB 重新触发一次总召
    Gi(String),
    PollPoints,
    PollDataSet(usize),
}

pub(super) struct Schedule {
    pub(super) job: Job,
    pub(super) every: Duration,
}

pub(super) struct Session {
    pub(super) conn: Arc<Connection>,
    dispatcher: Arc<Dispatcher>,
    poll_points: Vec<PollPoint>,
    datasets: Vec<PolledDataSet>,
    pub(super) schedule: Vec<Schedule>,
    /// 订阅成功的 RCB 数量
    pub(super) subscribed: usize,
}

/// `LD/LN.name` 或 `LD/LN$name` 形式的数据集引用: 展开 LD 的后缀, 统一成 MMS 的 `$` 写法。
fn resolve_dataset_ref(conn: &Connection, text: &str) -> Result<String, Error> {
    let (ld, rest) = text
        .trim()
        .split_once('/')
        .ok_or_else(|| Error::Reference(format!("{text:?} 缺少LD与LN之间的'/'")))?;
    Ok(format!(
        "{}/{}",
        conn.resolve_ld(ld)?,
        rest.replace('.', "$")
    ))
}

/// 连接丢失要交给上层重连; 其他错误只影响单个来源, 记录后降级。
fn fatal(err: &Error) -> bool {
    err.is_connection_lost()
}

impl Session {
    pub(super) fn establish(
        conn: Arc<Connection>,
        dev_id: &str,
        configs: &Iec61850Configs,
        default_interval: Duration,
    ) -> anyhow::Result<Session> {
        // 1. 点表里的引用解析成设备上真实的 LD 名
        let mut resolved = Vec::new();
        for cfg in configs.points.iter().filter(|p| p.kind.is_monitor()) {
            match conn.resolve_ref(&cfg.reference) {
                Ok(reference) => resolved.push((cfg.clone(), reference)),
                Err(e) if fatal(&e) => return Err(e.into()),
                Err(e) => warn!(
                    "[{dev_id}] 点 {} 的引用 {} 无法解析, 已忽略: {e}",
                    cfg.key, cfg.reference
                ),
            }
        }
        let monitored: Vec<(Fc, ObjectRef)> = resolved
            .iter()
            .filter_map(|(_, r)| r.fc.map(|fc| (fc, r.clone())))
            .collect();
        let dispatcher = Arc::new(Dispatcher::new(dev_id, resolved));

        // 2. 订阅报告 / 登记要轮询的数据集
        let mut layouts: HashMap<String, Arc<DataSetLayout>> = HashMap::new();
        let mut covering: Vec<Arc<DataSetLayout>> = Vec::new();
        let mut datasets: Vec<PolledDataSet> = Vec::new();
        let mut schedule: Vec<Schedule> = Vec::new();
        let mut subscribed = 0;
        let period = |ms: Option<u64>| {
            let d = ms.map(Duration::from_millis).unwrap_or(default_interval);
            (!d.is_zero()).then_some(d)
        };

        for src in &configs.sources {
            let label = format!("{:?} {}", src.kind, src.reference);
            // 布局只在订阅前查一次: 查询是同步请求, 不能放进报告回调里
            let mut layout_of = |ds_ref: &str| -> Result<Arc<DataSetLayout>, Error> {
                if let Some(l) = layouts.get(ds_ref) {
                    return Ok(l.clone());
                }
                let l = Arc::new(conn.prepare_data_set(ds_ref)?);
                layouts.insert(ds_ref.to_string(), l.clone());
                Ok(l)
            };
            let result: Result<(), Error> = (|| {
                match src.kind {
                    SourceKind::Urcb | SourceKind::Brcb => {
                        let candidates = conn.resolve_rcbs(&src.reference)?;
                        let data_set = src
                            .dataset
                            .as_deref()
                            .map(|d| resolve_dataset_ref(&conn, d))
                            .transpose()?;
                        let ds_ref = match &data_set {
                            Some(d) => d.clone(),
                            None => conn.rcb_data_set(&candidates[0])?,
                        };
                        let layout = layout_of(&ds_ref)?;
                        let gi_bit = if src.gi { TRG_GI } else { 0 };
                        let config = ReportConfig {
                            data_set,
                            trg_ops: src
                                .trg_ops
                                .unwrap_or(TRG_DATA_CHANGED | TRG_QUALITY_CHANGED)
                                | gi_bit,
                            intg_pd_ms: src.intg_pd_ms,
                            buf_tm_ms: src.buf_tm_ms,
                            general_interrogation: src.gi,
                            purge_buffer: src.purge,
                        };
                        let handler = {
                            let (dispatcher, layout) = (dispatcher.clone(), layout.clone());
                            Arc::new(move |report: &Report| {
                                dispatcher.ingest(
                                    report
                                        .entries
                                        .iter()
                                        .flat_map(|e| layout.leaves(e.index, &e.value)),
                                );
                            })
                        };
                        let chosen = conn.subscribe_report_any(&candidates, &config, handler)?;
                        info!(
                            "[{dev_id}] 已订阅 {chosen} (数据集 {ds_ref}, {} 个成员)",
                            layout.len()
                        );
                        if let Some(every) = period(src.interval_ms) {
                            schedule.push(Schedule {
                                job: Job::Gi(chosen),
                                every,
                            });
                        }
                        covering.push(layout);
                        subscribed += 1;
                    }
                    SourceKind::Dataset => {
                        let ds_ref = resolve_dataset_ref(&conn, &src.reference)?;
                        let layout = layout_of(&ds_ref)?;
                        info!("[{dev_id}] 轮询数据集 {ds_ref} ({} 个成员)", layout.len());
                        if let Some(every) = period(src.interval_ms) {
                            schedule.push(Schedule {
                                job: Job::PollDataSet(datasets.len()),
                                every,
                            });
                        }
                        covering.push(layout.clone());
                        datasets.push(PolledDataSet {
                            name: ds_ref,
                            layout,
                        });
                    }
                }
                Ok(())
            })();
            match result {
                Ok(()) => {}
                Err(e) if fatal(&e) => return Err(e.into()),
                Err(Error::RcbBusy(rcb)) => warn!(
                    "[{dev_id}] {label}: 所有候选 RCB 都被其他客户端占用({rcb}), 它覆盖的点改为轮询"
                ),
                Err(e) => warn!("[{dev_id}] {label} 不可用, 它覆盖的点改为轮询: {e}"),
            }
        }

        // 3. 没被任何成功订阅的 RCB 或轮询数据集覆盖的点, 逐点轮询兜底
        let poll_points: Vec<PollPoint> = monitored
            .into_iter()
            .filter(|(fc, r)| {
                !covering
                    .iter()
                    .any(|l| l.members().any(|m| m.covers(r) && m.fc == Some(*fc)))
            })
            .map(|(fc, reference)| PollPoint {
                dotted: reference.to_dotted(),
                reference,
                fc,
            })
            .collect();
        if !poll_points.is_empty() {
            let sample: Vec<String> = poll_points
                .iter()
                .take(5)
                .map(|p| p.reference.to_string())
                .collect();
            info!(
                "[{dev_id}] {} 个点不在任何已订阅的数据集里, 改为逐点轮询, 如: {}",
                poll_points.len(),
                sample.join(", ")
            );
            if let Some(every) = period(None) {
                schedule.push(Schedule {
                    job: Job::PollPoints,
                    every,
                });
            }
        }

        Ok(Session {
            conn,
            dispatcher,
            poll_points,
            datasets,
            schedule,
            subscribed,
        })
    }

    /// 连接刚建立时把轮询类的来源先读一遍(报告类由 GI 负责)。
    pub(super) fn initial_poll(&self) -> Result<(), Error> {
        self.run_job(&Job::PollPoints)?;
        (0..self.datasets.len()).try_for_each(|i| self.run_job(&Job::PollDataSet(i)))
    }

    /// 执行一个周期任务。连接丢失返回 `Err` 让上层重连; 其他错误记录后继续。
    pub(super) fn run_job(&self, job: &Job) -> Result<(), Error> {
        let dev = &self.dispatcher.dev_id;
        match job {
            Job::Gi(rcb) => match self.conn.trigger_gi(rcb) {
                Err(e) if fatal(&e) => return Err(e),
                Err(e) => warn!("[{dev}] 触发总召失败 {rcb}: {e}"),
                Ok(()) => debug!("[{dev}] 已触发总召 {rcb}"),
            },
            Job::PollPoints => {
                for p in &self.poll_points {
                    match self.conn.read(&p.dotted, p.fc) {
                        Ok(value) => self.dispatcher.ingest([(p.reference.clone(), value)]),
                        Err(e) if fatal(&e) => return Err(e),
                        Err(e) => debug!("[{dev}] 读取 {} 失败: {e}", p.reference),
                    }
                }
            }
            Job::PollDataSet(i) => {
                let ds = &self.datasets[*i];
                match self.conn.read_data_set(&ds.name) {
                    Ok(values) => self.dispatcher.ingest(
                        values
                            .iter()
                            .enumerate()
                            .flat_map(|(idx, v)| ds.layout.leaves(idx, v)),
                    ),
                    Err(e) if fatal(&e) => return Err(e),
                    Err(e) => warn!("[{dev}] 读取数据集 {} 失败: {e}", ds.name),
                }
            }
        }
        Ok(())
    }

    /// 用于下发后读回确认: 按监视点的配置读一次, 转成点值。
    pub(super) fn read_point(&self, cfg: &Iec61850Config) -> Option<crate::core::point::Val> {
        let reference = self.conn.resolve_ref(&cfg.reference).ok()?;
        let fc = reference.fc?;
        let value = self.conn.read(&reference.to_dotted(), fc).ok()?;
        decode_value(cfg, &value)
    }
}
