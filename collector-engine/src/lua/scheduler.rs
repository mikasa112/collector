use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use mlua::{Lua, LuaOptions, StdLib};
use tokio::{sync::mpsc, task::JoinHandle};
use tokio_util::sync::CancellationToken;

use collector_core::center::SharedPointCenter;

use crate::lua::{
    ScriptEngineError, api,
    loader::{Schedule, ScriptMeta, load_script},
    watcher::FileEvent,
};

struct TaskHandle {
    /// 取消标志，设置为 true 后任务会在下次循环退出
    cancel: Arc<AtomicBool>,
    join: JoinHandle<()>,
}

impl TaskHandle {
    fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub struct Scheduler {
    center: SharedPointCenter,
    /// path -> 正在运行的任务句柄
    tasks: HashMap<PathBuf, TaskHandle>,
}

impl Scheduler {
    pub fn new(center: SharedPointCenter) -> Self {
        Self {
            center,
            tasks: HashMap::new(),
        }
    }

    /// 加载脚本并启动定时任务
    pub fn spawn(&mut self, meta: ScriptMeta) {
        // 先停掉同路径的旧任务
        self.remove(&meta.path);

        let path = meta.path.clone();
        let cancel = Arc::new(AtomicBool::new(false));

        let join = match meta.schedule.clone() {
            Schedule::Cron(expr) => {
                spawn_cron_task(meta, self.center.clone(), cancel.clone(), expr)
            }
            Schedule::Interval(ms) => {
                spawn_interval_task(meta, self.center.clone(), cancel.clone(), ms)
            }
        };

        self.tasks.insert(path, TaskHandle { cancel, join });
    }

    /// 停止并移除任务
    pub fn remove(&mut self, path: &PathBuf) {
        if let Some(handle) = self.tasks.remove(path) {
            handle.cancel();
            handle.join.abort();
        }
    }

    /// 处理文件事件（热更新入口）
    pub async fn handle_event(&mut self, event: FileEvent) {
        match event {
            FileEvent::Upsert(path) => {
                tracing::info!("热更新: {}", path.display());
                match load_script(&path).await {
                    Ok(meta) => self.spawn(meta),
                    Err(err) => tracing::warn!("热更新失败 {}: {}", path.display(), err),
                }
            }
            FileEvent::Remove(path) => {
                tracing::info!("脚本已删除: {}", path.display());
                self.remove(&path);
            }
        }
    }

    /// 运行主循环：接收文件事件，收到关闭信号时停止所有任务
    pub async fn run(
        mut self,
        mut event_rx: mpsc::Receiver<FileEvent>,
        shutdown: CancellationToken,
    ) {
        loop {
            tokio::select! {
                Some(event) = event_rx.recv() => {
                    self.handle_event(event).await;
                }
                _ = shutdown.cancelled() => {
                    break;
                }
            }
        }
        // 停止所有脚本任务
        for (_, handle) in self.tasks.drain() {
            handle.cancel();
            handle.join.abort();
        }
    }
}

/// 创建并初始化一个 Lua VM，注册 API，执行脚本源码，调用 on_load
async fn build_vm(meta: &ScriptMeta, center: SharedPointCenter) -> Result<Lua, ScriptEngineError> {
    let lua = Lua::new_with(StdLib::ALL_SAFE, LuaOptions::default())
        .map_err(|e| ScriptEngineError::Lua(e.to_string()))?;

    api::register_all(&lua, center).map_err(|e| ScriptEngineError::Lua(e.to_string()))?;

    lua.load(&meta.source)
        .exec_async()
        .await
        .map_err(|e| ScriptEngineError::Lua(format!("{}: {}", meta.path.display(), e)))?;

    // 校验 on_tick 必须存在
    let has_tick: bool = lua
        .globals()
        .get::<Option<mlua::Function>>("on_tick")
        .map_err(|e| ScriptEngineError::Lua(e.to_string()))?
        .is_some();
    if !has_tick {
        return Err(ScriptEngineError::MissingField("on_tick".into()));
    }

    // 调用可选的 on_load 钩子
    if let Err(err) = api::call_hook(&lua, "on_load").await {
        tracing::warn!("[{}] on_load 错误: {}", meta.name, err);
    }

    Ok(lua)
}

fn spawn_interval_task(
    meta: ScriptMeta,
    center: SharedPointCenter,
    cancel: Arc<AtomicBool>,
    interval_ms: u64,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let lua = match build_vm(&meta, center).await {
            Ok(l) => l,
            Err(err) => {
                tracing::error!("[{}] 初始化失败: {}", meta.name, err);
                return;
            }
        };

        let mut ticker = tokio::time::interval(tokio::time::Duration::from_millis(interval_ms));
        let running = Arc::new(AtomicBool::new(false));

        loop {
            ticker.tick().await;

            if cancel.load(Ordering::Relaxed) {
                break;
            }

            // 跳过：上次还在运行
            if running.swap(true, Ordering::Relaxed) {
                tracing::warn!("[{}] 上次执行未完成，跳过本次调度", meta.name);
                continue;
            }

            let r = running.clone();
            let name = meta.name.clone();

            match call_on_tick(&lua).await {
                Ok(()) => {}
                Err(err) => tracing::error!("[{}] on_tick 错误: {}", name, err),
            }
            r.store(false, Ordering::Relaxed);
        }
    })
}

fn spawn_cron_task(
    meta: ScriptMeta,
    center: SharedPointCenter,
    cancel: Arc<AtomicBool>,
    cron_expr: String,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let lua = match build_vm(&meta, center).await {
            Ok(l) => l,
            Err(err) => {
                tracing::error!("[{}] 初始化失败: {}", meta.name, err);
                return;
            }
        };

        let schedule: cron::Schedule = match cron_expr.parse() {
            Ok(s) => s,
            Err(err) => {
                tracing::error!("[{}] cron 解析失败: {}", meta.name, err);
                return;
            }
        };

        let running = Arc::new(AtomicBool::new(false));

        for next in schedule.upcoming(chrono::Utc) {
            if cancel.load(Ordering::Relaxed) {
                break;
            }

            let now = chrono::Utc::now();
            let wait = match (next - now).to_std() {
                Ok(d) => d,
                Err(_) => {
                    // 时钟回拨或 next 已过期，跳过本次调度
                    tracing::warn!("[{}] 调度时间已过期（时钟回拨？），跳过", meta.name);
                    continue;
                }
            };
            tokio::time::sleep(wait).await;

            if cancel.load(Ordering::Relaxed) {
                break;
            }

            if running.swap(true, Ordering::Relaxed) {
                tracing::warn!("[{}] 上次执行未完成，跳过本次调度", meta.name);
                continue;
            }

            let r = running.clone();
            let name = meta.name.clone();

            match call_on_tick(&lua).await {
                Ok(()) => {}
                Err(err) => tracing::error!("[{}] on_tick 错误: {}", name, err),
            }
            r.store(false, Ordering::Relaxed);
        }
    })
}

async fn call_on_tick(lua: &Lua) -> mlua::Result<()> {
    let func: Option<mlua::Function> = lua.globals().get("on_tick")?;
    match func {
        Some(f) => f.call_async::<()>(()).await,
        None => Err(mlua::Error::runtime("脚本未定义 on_tick 函数")),
    }
}
