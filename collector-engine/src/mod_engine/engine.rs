use std::{
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use collector_core::center::SharedPointCenter;
use mlua::{Lua, LuaSerdeExt};
use parking_lot::RwLock;
use tokio::sync::{mpsc, oneshot};

use crate::mod_engine::{
    self,
    api::{dc::create_dc_table, log::create_log_table},
    errors::Error,
    eventbus::EventBus,
    scheduler::Scheduler,
};

// ── 命令枚举 ────────────────────────────────────────────────────────────────

pub enum EngineCmd {
    /// 触发一个事件，携带序列化后的 JSON 值（跨线程安全）
    Emit {
        name: String,
        value: serde_json::Value,
    },
    /// 加载并执行一段 Lua 脚本，通过 oneshot 回报成功/失败
    LoadScript {
        source: String,
        result_tx: oneshot::Sender<Result<(), String>>,
    },
    /// 关闭引擎循环
    Shutdown,
}

// ── 对外句柄（Clone，Send，Sync）────────────────────────────────────────────

/// 外部持有此句柄与引擎通信，可在任意 async task 中使用
#[derive(Clone)]
pub struct ModEngineHandle {
    tx: mpsc::UnboundedSender<EngineCmd>,
    /// 用于立即唤醒 run_blocking 中的 sleep，使 Shutdown 命令能即时响应
    waker: Arc<(Mutex<bool>, Condvar)>,
}

impl ModEngineHandle {
    pub fn emit(
        &self,
        name: impl Into<String>,
        value: serde_json::Value,
    ) -> mod_engine::Result<()> {
        self.tx
            .send(EngineCmd::Emit {
                name: name.into(),
                value,
            })
            .map_err(|_| Error::EngineClosed)
    }

    pub async fn load_script(&self, source: impl Into<String>) -> mod_engine::Result<()> {
        let (result_tx, result_rx) = oneshot::channel();
        self.tx
            .send(EngineCmd::LoadScript { source: source.into(), result_tx })
            .map_err(|_| Error::EngineClosed)?;
        result_rx
            .await
            .map_err(|_| Error::EngineClosed)?
            .map_err(Error::ScriptLoad)
    }

    pub fn shutdown(&self) {
        // 发命令 + 立即唤醒 run_blocking 的 sleep，使其不等满 100ms 就退出
        let _ = self.tx.send(EngineCmd::Shutdown);
        let (lock, cvar) = &*self.waker;
        *lock.lock().unwrap() = true;
        cvar.notify_one();
    }
}

// ── 内部引擎（单线程，持有 Lua VM）─────────────────────────────────────────

pub struct ModEngine {
    lua: Lua,
    events: Arc<RwLock<EventBus>>,
    scheduler: Arc<RwLock<Scheduler>>,
    rx: mpsc::UnboundedReceiver<EngineCmd>,
    waker: Arc<(Mutex<bool>, Condvar)>,
}

impl ModEngine {
    /// 创建引擎和对外句柄，引擎尚未启动
    pub fn create(center: SharedPointCenter) -> mod_engine::Result<(Self, ModEngineHandle)> {
        let (tx, rx) = mpsc::unbounded_channel();
        let waker = Arc::new((Mutex::new(false), Condvar::new()));
        let engine = Self {
            lua: mlua::Lua::new(),
            events: Arc::new(RwLock::new(EventBus::new())),
            scheduler: Arc::new(RwLock::new(Scheduler::new())),
            rx,
            waker: waker.clone(),
        };
        engine.register_api(center)?;
        engine.register_event()?;
        engine.register_timer()?;
        engine.register_task()?;
        let handle = ModEngineHandle { tx, waker };
        Ok((engine, handle))
    }

    fn register_api(&self, center: SharedPointCenter) -> mod_engine::Result<()> {
        let globals = self.lua.globals();
        globals.set("log", create_log_table(&self.lua)?)?;
        globals.set("dc", create_dc_table(&self.lua, center)?)?;
        Ok(())
    }

    fn register_event(&self) -> mod_engine::Result<()> {
        let globals = self.lua.globals();
        let event = self.lua.create_table()?;
        let events = self.events.clone();
        event.set(
            "on",
            self.lua
                .create_function(move |lua, (name, func): (String, mlua::Function)| {
                    let key = lua.create_registry_value(func)?;
                    events.write().handlers.entry(name).or_default().push(key);
                    Ok(())
                })?,
        )?;
        globals.set("event", event)?;
        Ok(())
    }

    fn register_timer(&self) -> mod_engine::Result<()> {
        let globals = self.lua.globals();
        let timer = self.lua.create_table()?;
        {
            let scheduler = self.scheduler.clone();
            timer.set(
                "after",
                self.lua
                    .create_function(move |lua, (ms, func): (u64, mlua::Function)| {
                        let key = lua.create_registry_value(func)?;
                        scheduler.write().add_after(Duration::from_millis(ms), key);
                        Ok(())
                    })?,
            )?;
        }
        {
            let scheduler = self.scheduler.clone();
            timer.set(
                "every",
                self.lua
                    .create_function(move |lua, (ms, func): (u64, mlua::Function)| {
                        let key = lua.create_registry_value(func)?;
                        scheduler.write().add_every(Duration::from_millis(ms), key);
                        Ok(())
                    })?,
            )?;
        }
        globals.set("timer", timer)?;
        Ok(())
    }

    fn register_task(&self) -> mod_engine::Result<()> {
        let globals = self.lua.globals();

        // wait(ms) 就是 coroutine.yield(ms)，纯 Lua 实现避免跨 C 边界 yield 限制
        self.lua
            .load(
                r#"
            function wait(ms)
                coroutine.yield(ms)
            end
        "#,
            )
            .exec()?;

        let task_table = self.lua.create_table()?;
        let scheduler = self.scheduler.clone();
        task_table.set(
            "spawn",
            self.lua.create_function(move |lua, func: mlua::Function| {
                let thread = lua.create_thread(func)?;
                scheduler.write().add_coroutine(thread);
                Ok(())
            })?,
        )?;
        globals.set("task", task_table)?;
        Ok(())
    }

    // ── 处理 channel 中的待处理命令（非阻塞排空）──────────────────────────

    fn drain_commands(&mut self) -> mod_engine::Result<bool> {
        loop {
            match self.rx.try_recv() {
                Ok(cmd) => match cmd {
                    EngineCmd::Emit { name, value } => {
                        self.emit_inner(&name, value)?;
                    }
                    EngineCmd::LoadScript { source, result_tx } => {
                        let result = self.lua.load(&source).exec()
                            .map_err(|e| e.to_string());
                        let _ = result_tx.send(result);
                    }
                    EngineCmd::Shutdown => return Ok(true),
                },
                Err(mpsc::error::TryRecvError::Empty) => return Ok(false),
                Err(mpsc::error::TryRecvError::Disconnected) => return Ok(true),
            }
        }
    }

    fn emit_inner(&self, name: &str, value: serde_json::Value) -> mod_engine::Result<()> {
        // 先把回调函数列表克隆出来再释放读锁，
        // 防止回调内部调用 event.on() 请求写锁造成死锁
        let funcs: Vec<mlua::Function> = {
            let binding = self.events.read();
            let Some(list) = binding.handlers.get(name) else {
                return Ok(());
            };
            list.iter()
                .filter_map(|k| self.lua.registry_value::<mlua::Function>(k).ok())
                .collect()
        };
        let lua_val = self.lua.to_value(&value)?;
        for func in funcs {
            func.call::<()>(lua_val.clone())?;
        }
        Ok(())
    }

    // ── 主循环（在专用阻塞线程中调用）────────────────────────────────────

    /// 阻塞运行直到收到 Shutdown 命令或 sender 全部 drop
    pub fn run_blocking(mut self) -> mod_engine::Result<()> {
        let (lock, cvar) = &*self.waker.clone();
        loop {
            if self.drain_commands()? {
                break;
            }

            self.scheduler.write().tick(&self.lua)?;

            let sleep_dur = {
                let sched = self.scheduler.read();
                sched
                    .next_wake()
                    .map(|wake| {
                        wake.saturating_duration_since(tokio::time::Instant::now())
                            .min(Duration::from_millis(100))
                    })
                    .unwrap_or(Duration::from_millis(100))
            };

            // 用 condvar 替代 thread::sleep，shutdown 时可被立即唤醒
            let guard = lock.lock().unwrap();
            let _ = cvar.wait_timeout_while(guard, sleep_dur, |woken| !*woken);
        }
        tracing::info!("[mod] 引擎已关闭");
        Ok(())
    }
}
