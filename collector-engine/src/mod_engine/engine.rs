use std::{sync::Arc, time::Duration};

use collector_core::center::SharedPointCenter;
use mlua::{Lua, LuaSerdeExt};
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
    Emit {
        name: String,
        value: serde_json::Value,
    },
    LoadScript {
        source: String,
        result_tx: oneshot::Sender<Result<(), String>>,
    },
    AddTimer {
        delay: Duration,
        interval: Option<Duration>,
        callback: mlua::RegistryKey,
    },
    AddCoroutine {
        thread: mlua::Thread,
    },
    Shutdown,
}

// ── 对外句柄（Clone，Send，Sync）────────────────────────────────────────────

#[derive(Clone)]
pub struct ModEngineHandle {
    tx: mpsc::UnboundedSender<EngineCmd>,
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
            .send(EngineCmd::LoadScript {
                source: source.into(),
                result_tx,
            })
            .map_err(|_| Error::EngineClosed)?;
        result_rx
            .await
            .map_err(|_| Error::EngineClosed)?
            .map_err(Error::ScriptLoad)
    }

    pub fn shutdown(&self) {
        let _ = self.tx.send(EngineCmd::Shutdown);
    }
}

// ── 内部引擎 ────────────────────────────────────────────────────────────────

pub struct ModEngine {
    lua: Lua,
    events: Arc<std::sync::RwLock<EventBus>>,
    scheduler: Scheduler,
    rx: mpsc::UnboundedReceiver<EngineCmd>,
}

impl ModEngine {
    pub fn create(center: SharedPointCenter) -> mod_engine::Result<(Self, ModEngineHandle)> {
        let (tx, rx) = mpsc::unbounded_channel();
        let engine = Self {
            lua: Lua::new(),
            events: Arc::new(std::sync::RwLock::new(EventBus::new())),
            scheduler: Scheduler::new(),
            rx,
        };
        engine.register_api(center)?;
        engine.register_event()?;
        engine.register_timer(tx.clone())?;
        engine.register_task(tx.clone())?;
        let handle = ModEngineHandle { tx };
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
                    events
                        .write()
                        .unwrap()
                        .handlers
                        .entry(name)
                        .or_default()
                        .push(key);
                    Ok(())
                })?,
        )?;
        globals.set("event", event)?;
        Ok(())
    }

    fn register_timer(&self, tx: mpsc::UnboundedSender<EngineCmd>) -> mod_engine::Result<()> {
        let globals = self.lua.globals();
        let timer = self.lua.create_table()?;
        {
            let tx2 = tx.clone();
            timer.set(
                "after",
                self.lua
                    .create_function(move |lua, (ms, func): (u64, mlua::Function)| {
                        let key = lua.create_registry_value(func)?;
                        let _ = tx2.send(EngineCmd::AddTimer {
                            delay: Duration::from_millis(ms),
                            interval: None,
                            callback: key,
                        });
                        Ok(())
                    })?,
            )?;
        }
        {
            timer.set(
                "every",
                self.lua
                    .create_function(move |lua, (ms, func): (u64, mlua::Function)| {
                        let key = lua.create_registry_value(func)?;
                        let _ = tx.send(EngineCmd::AddTimer {
                            delay: Duration::from_millis(ms),
                            interval: Some(Duration::from_millis(ms)),
                            callback: key,
                        });
                        Ok(())
                    })?,
            )?;
        }
        globals.set("timer", timer)?;
        Ok(())
    }

    fn register_task(&self, tx: mpsc::UnboundedSender<EngineCmd>) -> mod_engine::Result<()> {
        let globals = self.lua.globals();

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
        task_table.set(
            "spawn",
            self.lua.create_function(move |lua, func: mlua::Function| {
                let thread = lua.create_thread(func)?;
                let _ = tx.send(EngineCmd::AddCoroutine { thread });
                Ok(())
            })?,
        )?;
        globals.set("task", task_table)?;
        Ok(())
    }

    /// 处理单条命令，返回 true 表示应退出
    async fn process_cmd(&mut self, cmd: EngineCmd) -> mod_engine::Result<bool> {
        match cmd {
            EngineCmd::Emit { name, value } => {
                self.emit_inner(&name, value).await?;
            }
            EngineCmd::LoadScript { source, result_tx } => {
                let result = self
                    .lua
                    .load(&source)
                    .exec_async()
                    .await
                    .map_err(|e| e.to_string());
                let _ = result_tx.send(result);
            }
            EngineCmd::AddTimer {
                delay,
                interval,
                callback,
            } => {
                if let Some(interval) = interval {
                    self.scheduler.add_every(interval, callback);
                } else {
                    self.scheduler.add_after(delay, callback);
                }
            }
            EngineCmd::AddCoroutine { thread } => {
                self.scheduler.add_coroutine(thread);
            }
            EngineCmd::Shutdown => return Ok(true),
        }
        Ok(false)
    }

    async fn drain_commands(&mut self) -> mod_engine::Result<bool> {
        loop {
            match self.rx.try_recv() {
                Ok(cmd) => {
                    if self.process_cmd(cmd).await? {
                        return Ok(true);
                    }
                }
                Err(mpsc::error::TryRecvError::Empty) => return Ok(false),
                Err(mpsc::error::TryRecvError::Disconnected) => return Ok(true),
            }
        }
    }

    async fn emit_inner(&self, name: &str, value: serde_json::Value) -> mod_engine::Result<()> {
        let funcs: Vec<mlua::Function> = {
            let binding = self.events.read().unwrap();
            let Some(list) = binding.handlers.get(name) else {
                return Ok(());
            };
            list.iter()
                .filter_map(|k| self.lua.registry_value::<mlua::Function>(k).ok())
                .collect()
        };
        let lua_val = self.lua.to_value(&value)?;
        for func in funcs {
            func.call_async::<()>(lua_val.clone()).await?;
        }
        Ok(())
    }

    /// 异步运行直到收到 Shutdown 命令或 sender 全部 drop
    pub async fn run(mut self) -> mod_engine::Result<()> {
        loop {
            if self.drain_commands().await? {
                break;
            }

            self.scheduler.tick(&self.lua).await?;

            let sleep_dur = self
                .scheduler
                .next_wake()
                .map(|wake| {
                    wake.saturating_duration_since(tokio::time::Instant::now())
                        .min(Duration::from_millis(100))
                })
                .unwrap_or(Duration::from_millis(100));

            // select! 让新命令（尤其是 Shutdown）能立即打断休眠
            tokio::select! {
                _ = tokio::time::sleep(sleep_dur) => {}
                cmd = self.rx.recv() => {
                    match cmd {
                        None => break,
                        Some(cmd) => {
                            if self.process_cmd(cmd).await? {
                                break;
                            }
                        }
                    }
                }
            }
        }
        tracing::info!("[mod] 引擎已关闭");
        Ok(())
    }
}
