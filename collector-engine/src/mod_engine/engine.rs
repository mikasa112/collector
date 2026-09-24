use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, atomic::AtomicU64},
    time::Duration,
};

use collector_core::{
    core::point::DataPoint, dev::can_bus::SharedCanBus, dock::mqtt::MqttOverrideStore,
};
use mlua::{Lua, LuaSerdeExt};
use tokio::sync::{mpsc, oneshot};

use crate::mod_engine::{
    self,
    api::{
        can::create_can_table,
        dc::create_dc_table,
        json::create_json_table,
        log::create_log_table,
        mqtt::create_mqtt_table,
        mqtt_client::{ConnEntry, MqttConns, MqttSubs, create_mqtt_conn_table, topic_matches},
        plugin::create_plugin_table,
        save::create_save_table,
        store::{LuaStore, create_store_table},
    },
    budget::{Budget, HOOK_INSTRUCTION_COUNT},
    errors::Error,
    eventbus::EventBus,
    global_bus::GlobalBus,
    hook::HookRegistry,
    introspect::{EngineSnapshot, EventInfo, HookInfo, MqttSubInfo},
    scheduler::{Scheduler, SchedulerStats},
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
    MqttMessage {
        conn_id: u64,
        topic: String,
        payload: Vec<u8>,
    },
    /// 跨 VM 请求：由 `event.request` 触发，投递到目标脚本的 VM，处理结果通过 `CrossResponse` 送回 `from`
    CrossRequest {
        req_id: u64,
        from: ModEngineHandle,
        name: String,
        value: serde_json::Value,
    },
    /// 跨 VM 请求的响应：投递回发起方 VM，写入 `__pending_responses` 供其协程轮询取走
    CrossResponse {
        req_id: u64,
        result: Result<serde_json::Value, String>,
    },
    /// 运行时自省查询：由 `sys.status(其他脚本名)` 或 `GlobalBus::snapshot_all()` 发起，
    /// 只用于查询**别的**引擎——查自己走本地直读 Arc，绝不能把这条命令发给自己（见 register_sys 注释）
    Introspect {
        result_tx: oneshot::Sender<EngineSnapshot>,
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

    /// 向本引擎投递一个跨 VM 请求（由请求方持有目标引擎的 handle 调用）
    pub fn cross_request(
        &self,
        from: ModEngineHandle,
        req_id: u64,
        name: String,
        value: serde_json::Value,
    ) -> mod_engine::Result<()> {
        self.tx
            .send(EngineCmd::CrossRequest {
                req_id,
                from,
                name,
                value,
            })
            .map_err(|_| Error::EngineClosed)
    }

    /// 向本引擎投递一个跨 VM 请求的响应（由目标引擎持有请求方的 handle 调用）
    pub fn cross_response(
        &self,
        req_id: u64,
        result: Result<serde_json::Value, String>,
    ) -> mod_engine::Result<()> {
        self.tx
            .send(EngineCmd::CrossResponse { req_id, result })
            .map_err(|_| Error::EngineClosed)
    }

    /// 查询本引擎的运行时快照（事件/钩子订阅、定时器/协程数量、MQTT 连接/订阅）。
    /// 调用方与本引擎须是不同的 tokio 任务，否则会永久死锁——查自己应改用本地直读 Arc 的路径。
    pub async fn introspect(&self) -> mod_engine::Result<EngineSnapshot> {
        let (result_tx, result_rx) = oneshot::channel();
        self.tx
            .send(EngineCmd::Introspect { result_tx })
            .map_err(|_| Error::EngineClosed)?;
        result_rx.await.map_err(|_| Error::EngineClosed)
    }
}

// ── 内部引擎 ────────────────────────────────────────────────────────────────

pub struct ModEngine {
    lua: Lua,
    events: Arc<std::sync::RwLock<EventBus>>,
    hooks: Arc<std::sync::RwLock<HookRegistry>>,
    /// `event.on_request` 注册的跨 VM 请求处理器：请求名 -> 处理函数，同名重复注册覆盖旧的
    requests: Arc<std::sync::RwLock<HashMap<String, mlua::RegistryKey>>>,
    /// 单次 Lua 调用的执行时间预算，配合全局指令钩子防止脚本死循环卡死引擎
    budget: Budget,
    scheduler: Scheduler,
    /// `scheduler` 内部 timer/coroutine 数量的旁路计数，供自省 API 同步读取（无需 `&Scheduler`）
    scheduler_stats: SchedulerStats,
    rx: mpsc::UnboundedReceiver<EngineCmd>,
    dc_changed_rx: mpsc::UnboundedReceiver<(String, Arc<[DataPoint]>)>,
    mqtt_subs: MqttSubs,
    mqtt_conns: MqttConns,
    mqtt_next_id: Arc<AtomicU64>,
}

/// 拼装运行时自省快照的公共逻辑，被 `ModEngine::build_snapshot`（跨引擎查询路径，process_cmd
/// 里处理 `Introspect` 命令时用）和 `register_sys` 里 `sys.status()` 自查路径（本地直读 Arc，
/// 不经命令队列）共用，避免两处重复写字段读取代码。
fn build_snapshot_from_parts(
    events: &Arc<std::sync::RwLock<EventBus>>,
    requests: &Arc<std::sync::RwLock<HashMap<String, mlua::RegistryKey>>>,
    hooks: &Arc<std::sync::RwLock<HookRegistry>>,
    mqtt_subs: &MqttSubs,
    mqtt_conns: &MqttConns,
    stats: &SchedulerStats,
) -> EngineSnapshot {
    let events: Vec<EventInfo> = events
        .read()
        .unwrap()
        .handlers
        .iter()
        .map(|(name, list)| EventInfo {
            name: name.clone(),
            handlers: list.len(),
        })
        .collect();
    let requests: Vec<String> = requests.read().unwrap().keys().cloned().collect();
    let (hooks_on, hooks_override): (Vec<HookInfo>, Vec<String>) = {
        let binding = hooks.read().unwrap();
        let hooks_on = binding
            .on
            .iter()
            .map(|(name, list)| HookInfo {
                name: name.clone(),
                handlers: list.len(),
            })
            .collect();
        let hooks_override = binding.override_.keys().cloned().collect();
        (hooks_on, hooks_override)
    };
    let (timers, coroutines) = stats.snapshot();
    let mqtt_conns_count = mqtt_conns.lock().unwrap().len();
    let mqtt_subs_info: Vec<MqttSubInfo> = mqtt_subs
        .lock()
        .unwrap()
        .iter()
        .map(|s| MqttSubInfo {
            conn_id: s.conn_id,
            filter: s.filter.clone(),
        })
        .collect();
    EngineSnapshot {
        events,
        requests,
        hooks_on,
        hooks_override,
        timers,
        coroutines,
        mqtt_conns: mqtt_conns_count,
        mqtt_subs: mqtt_subs_info,
    }
}

/// `register_api` 入参打包，避免函数参数过多（clippy::too_many_arguments）
struct RegisterApiArgs {
    override_store: Option<MqttOverrideStore>,
    owned_topics: Arc<Mutex<Vec<String>>>,
    store: LuaStore,
    watch_tx: mpsc::UnboundedSender<(String, Arc<[DataPoint]>)>,
    can_bus: Option<SharedCanBus>,
    cmd_tx: mpsc::UnboundedSender<EngineCmd>,
    data_file: PathBuf,
    script_dir: PathBuf,
}

impl ModEngine {
    pub fn create(
        override_store: Option<MqttOverrideStore>,
        owned_topics: Arc<Mutex<Vec<String>>>,
        store: LuaStore,
        can_bus: Option<SharedCanBus>,
        script_dir: PathBuf,
        data_file: PathBuf,
        bus: GlobalBus,
        name: String,
    ) -> mod_engine::Result<(Self, ModEngineHandle)> {
        let (tx, rx) = mpsc::unbounded_channel();
        let (watch_tx, dc_changed_rx) = mpsc::unbounded_channel();
        let lua = Lua::new();
        let budget = Budget::new();
        // 在任何脚本代码跑起来之前装好全局指令钩子：此后新建的所有线程（call_async/exec_async
        // 内部线程、task.spawn 协程）都会自动沿用这个钩子，无需逐个线程单独设置
        {
            let budget = budget.clone();
            lua.set_global_hook(
                mlua::HookTriggers::new().every_nth_instruction(HOOK_INSTRUCTION_COUNT),
                move |_lua, _debug| {
                    if budget.is_expired() {
                        Err(mlua::Error::runtime("[mod] 脚本执行超时（可能陷入死循环），已被中断"))
                    } else {
                        Ok(mlua::VmState::Continue)
                    }
                },
            )?;
        }
        let handle = ModEngineHandle { tx: tx.clone() };
        let scheduler = Scheduler::new(budget.clone());
        let scheduler_stats = scheduler.stats_handle();
        let engine = Self {
            lua,
            events: Arc::new(std::sync::RwLock::new(EventBus::new())),
            hooks: Arc::new(std::sync::RwLock::new(HookRegistry::new())),
            requests: Arc::new(std::sync::RwLock::new(HashMap::new())),
            budget: budget.clone(),
            scheduler,
            scheduler_stats,
            rx,
            dc_changed_rx,
            mqtt_subs: Arc::new(Mutex::new(Vec::new())),
            mqtt_conns: Arc::new(Mutex::new(Vec::new())),
            mqtt_next_id: Arc::new(AtomicU64::new(1)),
        };
        engine.register_require(&script_dir)?;
        engine.register_api(RegisterApiArgs {
            override_store,
            owned_topics,
            store,
            watch_tx,
            can_bus,
            cmd_tx: tx.clone(),
            data_file,
            script_dir,
        })?;
        engine.register_event(bus.clone(), handle.clone())?;
        engine.register_hook()?;
        engine.register_timer(tx.clone())?;
        engine.register_task(tx.clone())?;
        engine.register_sys(bus, name)?;
        Ok((engine, handle))
    }

    /// 将 `require` 的搜索路径限定到脚本目录本身，使脚本可通过
    /// `require("_utils")` 之类的方式引入同目录下的公共 .lua 模块。
    /// 被 require 的模块运行在与调用脚本相同的 Lua VM 中，可直接使用 dc/log 等全局 API。
    fn register_require(&self, script_dir: &std::path::Path) -> mod_engine::Result<()> {
        let package: mlua::Table = self.lua.globals().get("package")?;
        package.set("path", format!("{}/?.lua", script_dir.display()))?;
        Ok(())
    }

    fn register_api(&self, args: RegisterApiArgs) -> mod_engine::Result<()> {
        let RegisterApiArgs {
            override_store,
            owned_topics,
            store,
            watch_tx,
            can_bus,
            cmd_tx,
            data_file,
            script_dir,
        } = args;
        let globals = self.lua.globals();
        globals.set("log", create_log_table(&self.lua)?)?;
        globals.set("json", create_json_table(&self.lua)?)?;
        globals.set("dc", create_dc_table(&self.lua, watch_tx)?)?;
        globals.set("store", create_store_table(&self.lua, store)?)?;
        globals.set("save", create_save_table(&self.lua, data_file)?)?;
        globals.set("plugin", create_plugin_table(&self.lua, script_dir)?)?;
        if let Some(mqtt_store) = override_store {
            globals.set(
                "override",
                create_mqtt_table(&self.lua, mqtt_store, owned_topics)?,
            )?;
        }
        if let Some(bus) = can_bus {
            globals.set("can", create_can_table(&self.lua, bus)?)?;
        }
        // mqtt：脚本自行发起独立连接，始终可用，不依赖 project 的 mqtt 配置
        globals.set(
            "mqtt",
            create_mqtt_conn_table(
                &self.lua,
                cmd_tx,
                self.mqtt_subs.clone(),
                self.mqtt_conns.clone(),
                self.mqtt_next_id.clone(),
            )?,
        )?;
        Ok(())
    }

    /// 注册全局 `event` 表：`on` 是本 VM 内的本地订阅（沿用已有 `EventBus`），
    /// `emit` 通过 `GlobalBus` 广播给所有仍在运行的顶层脚本（包括自己），
    /// 由各自 VM 本地的 `on` 处理器接收——实现顶层脚本间的跨 VM 协作通信。
    /// `on_request`/`request` 实现跨 VM 请求-响应：`request` 只能在协程上下文调用
    /// （`task.spawn`、`event.on`/`timer`/`mqtt` 等回调，均由 `call_async`/`exec_async`
    /// 派发，内部会创建协程线程），不能在 `hook.on`/`hook.override`（同步 `call`）里用。
    fn register_event(&self, bus: GlobalBus, self_handle: ModEngineHandle) -> mod_engine::Result<()> {
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
        let bus2 = bus.clone();
        event.set(
            "emit",
            self.lua
                .create_function(move |lua, (name, value): (String, mlua::Value)| {
                    let json: serde_json::Value = lua.from_value(value)?;
                    bus.broadcast(&name, json);
                    Ok(())
                })?,
        )?;

        let requests = self.requests.clone();
        event.set(
            "on_request",
            self.lua
                .create_function(move |lua, (name, func): (String, mlua::Function)| {
                    let key = lua.create_registry_value(func)?;
                    if requests.write().unwrap().insert(name.clone(), key).is_some() {
                        tracing::warn!("[mod] 请求处理器 {} 被重复注册，旧处理器已失效", name);
                    }
                    Ok(())
                })?,
        )?;

        event.set(
            "__send_request",
            self.lua.create_function(
                move |lua, (target, name, payload, req_id): (String, String, mlua::Value, u64)| {
                    let json: serde_json::Value = lua.from_value(payload)?;
                    match bus2.find_by_name(&target) {
                        Some(target_handle) => {
                            let _ =
                                target_handle.cross_request(self_handle.clone(), req_id, name, json);
                        }
                        None => {
                            let pending: mlua::Table = lua.globals().get("__pending_responses")?;
                            let entry = lua.create_table()?;
                            entry.set("ok", false)?;
                            entry.set("error", format!("目标模块 {} 不存在或未运行", target))?;
                            pending.set(req_id, entry)?;
                        }
                    }
                    Ok(())
                },
            )?,
        )?;

        globals.set("event", event)?;

        // 请求方轮询等待响应：响应通过 __pending_responses 表传递（而不是靠协程 resume 带回新值——
        // AsyncThread 的 resume 只会把上次 yield 的值原样喂回，见 mlua thread.rs 的 Stream 实现），
        // 故复用现成的 wait(ms) 做轮询，无需改动协程调度器。
        self.lua
            .load(
                r#"
            __pending_responses = {}
            __request_id_counter = 0

            function event.request(target, name, payload, timeout_ms)
                timeout_ms = timeout_ms or 5000
                __request_id_counter = __request_id_counter + 1
                local req_id = __request_id_counter
                event.__send_request(target, name, payload, req_id)
                local waited = 0
                local poll_interval = 20
                while __pending_responses[req_id] == nil do
                    wait(poll_interval)
                    waited = waited + poll_interval
                    if waited >= timeout_ms then
                        __pending_responses[req_id] = nil
                        error(string.format("[mod] event.request 超时: 目标 %s 在 %dms 内未响应", target, timeout_ms))
                    end
                end
                local resp = __pending_responses[req_id]
                __pending_responses[req_id] = nil
                if resp.ok then
                    return resp.value
                else
                    error("[mod] event.request 处理出错: " .. tostring(resp.error))
                end
            end
        "#,
            )
            .exec()?;
        Ok(())
    }

    /// 注册全局 `hook` 表：同 VM 内插件钩子机制。
    /// `hook.on` 允许多个扩展处理器（仅副作用，返回值忽略）；`hook.override` 同一钩子名只保留最后注册者（有返回值）；
    /// `hook.emit` 先同步跑完所有 `on` 处理器，再跑 `override`（有则用其返回值，无则原样返回 payload）。
    fn register_hook(&self) -> mod_engine::Result<()> {
        let globals = self.lua.globals();
        let hook_table = self.lua.create_table()?;

        {
            let hooks = self.hooks.clone();
            hook_table.set(
                "on",
                self.lua
                    .create_function(move |lua, (name, func): (String, mlua::Function)| {
                        let key = lua.create_registry_value(func)?;
                        hooks.write().unwrap().on.entry(name).or_default().push(key);
                        Ok(())
                    })?,
            )?;
        }

        {
            let hooks = self.hooks.clone();
            hook_table.set(
                "override",
                self.lua
                    .create_function(move |lua, (name, func): (String, mlua::Function)| {
                        let key = lua.create_registry_value(func)?;
                        if hooks.write().unwrap().override_.insert(name.clone(), key).is_some() {
                            tracing::warn!("[mod] 钩子 {} 的替换处理器被重复注册，旧处理器已失效", name);
                        }
                        Ok(())
                    })?,
            )?;
        }

        {
            let hooks = self.hooks.clone();
            hook_table.set(
                "emit",
                self.lua
                    .create_function(move |lua, (name, payload): (String, mlua::Value)| {
                        let (on_funcs, override_func): (Vec<mlua::Function>, Option<mlua::Function>) = {
                            let binding = hooks.read().unwrap();
                            let on_funcs = binding
                                .on
                                .get(&name)
                                .map(|list| {
                                    list.iter()
                                        .filter_map(|k| lua.registry_value::<mlua::Function>(k).ok())
                                        .collect()
                                })
                                .unwrap_or_default();
                            let override_func = binding
                                .override_
                                .get(&name)
                                .and_then(|k| lua.registry_value::<mlua::Function>(k).ok());
                            (on_funcs, override_func)
                        };
                        for func in on_funcs {
                            if let Err(e) = func.call::<()>(payload.clone()) {
                                tracing::warn!("[mod] 钩子 {} 的扩展处理器执行出错: {}", name, e);
                            }
                        }
                        match override_func {
                            Some(func) => func.call::<mlua::Value>(payload),
                            None => Ok(payload),
                        }
                    })?,
            )?;
        }

        globals.set("hook", hook_table)?;
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
                self.budget.reset();
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
            EngineCmd::MqttMessage {
                conn_id,
                topic,
                payload,
            } => {
                self.emit_mqtt_message(conn_id, &topic, &payload).await?;
            }
            EngineCmd::CrossRequest {
                req_id,
                from,
                name,
                value,
            } => {
                self.handle_cross_request(req_id, from, name, value).await?;
            }
            EngineCmd::CrossResponse { req_id, result } => {
                self.deliver_response(req_id, result)?;
            }
            EngineCmd::Introspect { result_tx } => {
                let _ = result_tx.send(self.build_snapshot());
            }
            EngineCmd::Shutdown => {
                self.run_lifecycle_hook("on_unload").await;
                self.shutdown_mqtt_conns().await;
                return Ok(true);
            }
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
            self.budget.reset();
            if let Err(e) = func.call_async::<()>(lua_val.clone()).await {
                tracing::warn!("[mod] 事件 {} 的处理器执行出错: {}", name, e);
            }
        }
        Ok(())
    }

    async fn emit_dc_changed(
        &self,
        dev_id: &str,
        snapshot: &Arc<[DataPoint]>,
    ) -> mod_engine::Result<()> {
        let funcs: Vec<mlua::Function> = {
            let binding = self.events.read().unwrap();
            let Some(list) = binding.handlers.get("dc:changed") else {
                return Ok(());
            };
            list.iter()
                .filter_map(|k| self.lua.registry_value::<mlua::Function>(k).ok())
                .collect()
        };
        if funcs.is_empty() {
            return Ok(());
        }
        let points: Vec<serde_json::Value> = snapshot
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.id,
                    "key": p.key,
                    "name": p.name,
                    "value": serde_json::to_value(&p.value).unwrap_or_default(),
                })
            })
            .collect();
        let payload = serde_json::json!({ "dev": dev_id, "points": points });
        let lua_val = self.lua.to_value(&payload)?;
        for func in funcs {
            self.budget.reset();
            if let Err(e) = func.call_async::<()>(lua_val.clone()).await {
                tracing::warn!("[mod] dc:changed 处理器执行出错: {}", e);
            }
        }
        Ok(())
    }

    async fn emit_mqtt_message(
        &self,
        conn_id: u64,
        topic: &str,
        payload: &[u8],
    ) -> mod_engine::Result<()> {
        let funcs: Vec<mlua::Function> = {
            let subs = self.mqtt_subs.lock().unwrap();
            subs.iter()
                .filter(|s| s.conn_id == conn_id && topic_matches(&s.filter, topic))
                .filter_map(|s| self.lua.registry_value::<mlua::Function>(&s.callback).ok())
                .collect()
        };
        if funcs.is_empty() {
            return Ok(());
        }
        let topic_val = self.lua.create_string(topic)?;
        let payload_val = self.lua.create_string(payload)?;
        for func in funcs {
            self.budget.reset();
            if let Err(e) = func
                .call_async::<()>((topic_val.clone(), payload_val.clone()))
                .await
            {
                tracing::warn!("[mod] mqtt 订阅回调执行出错: {}", e);
            }
        }
        Ok(())
    }

    /// 处理跨 VM 请求：查找本 VM 通过 `event.on_request` 注册的处理器并执行，
    /// 无论成功、出错还是未注册处理器，都把结果转成响应发回 `from`（不用 `?` 上抛，
    /// 避免请求方永久等到超时——处理失败应该是"快速返回一个错误响应"而不是让本引擎的 run() 循环终止）。
    async fn handle_cross_request(
        &self,
        req_id: u64,
        from: ModEngineHandle,
        name: String,
        value: serde_json::Value,
    ) -> mod_engine::Result<()> {
        let func: Option<mlua::Function> = {
            let binding = self.requests.read().unwrap();
            binding
                .get(&name)
                .and_then(|k| self.lua.registry_value::<mlua::Function>(k).ok())
        };
        let result = match func {
            Some(func) => {
                let lua_val = self.lua.to_value(&value)?;
                self.budget.reset();
                match func.call_async::<mlua::Value>(lua_val).await {
                    Ok(ret) => self
                        .lua
                        .from_value::<serde_json::Value>(ret)
                        .map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                }
            }
            None => Err(format!("[mod] 未注册请求处理器: {}", name)),
        };
        let _ = from.cross_response(req_id, result);
        Ok(())
    }

    /// 把跨 VM 请求的响应写入本 VM 的 `__pending_responses[req_id]`，供 `event.request` 的轮询取走
    fn deliver_response(
        &self,
        req_id: u64,
        result: Result<serde_json::Value, String>,
    ) -> mod_engine::Result<()> {
        let pending: mlua::Table = self.lua.globals().get("__pending_responses")?;
        let entry = self.lua.create_table()?;
        match result {
            Ok(v) => {
                entry.set("ok", true)?;
                entry.set("value", self.lua.to_value(&v)?)?;
            }
            Err(e) => {
                entry.set("ok", false)?;
                entry.set("error", e)?;
            }
        }
        pending.set(req_id, entry)?;
        Ok(())
    }

    /// 拼装本引擎的运行时自省快照，供 `process_cmd` 处理 `Introspect` 命令
    /// （跨引擎查询路径）时调用；自查路径见 `register_sys` 里的 `build_snapshot_from_parts`。
    fn build_snapshot(&self) -> EngineSnapshot {
        build_snapshot_from_parts(
            &self.events,
            &self.requests,
            &self.hooks,
            &self.mqtt_subs,
            &self.mqtt_conns,
            &self.scheduler_stats,
        )
    }

    /// 注册全局 `sys` 表：`list_scripts` 列出所有运行中的顶层脚本；`status(name?)` 查询自省快照。
    ///
    /// **死锁警告**：`status` 查询自己（不传参或传自己的 `MOD.name`）时必须走本地直读 Arc 的路径，
    /// 绝对不能把 `EngineCmd::Introspect` 发给自己的 `tx` 再 `.await` 结果——`status` 是在本引擎
    /// `run()` 循环内某个协程同步调用的，此时循环正阻塞在这次调用上，不会再回去 `self.rx.recv()`
    /// 处理刚发给自己的命令，会永久死锁，且这个死锁发生在 `.await` 一个 channel 上，不是死循环执行
    /// 指令，全局指令钩子超时保护完全捕捉不到。查询**别的**脚本才走命令队列 + oneshot 往返。
    fn register_sys(&self, bus: GlobalBus, own_name: String) -> mod_engine::Result<()> {
        let globals = self.lua.globals();
        let sys = self.lua.create_table()?;

        let bus_list = bus.clone();
        sys.set(
            "list_scripts",
            self.lua
                .create_function(move |lua, ()| lua.to_value(&bus_list.list()))?,
        )?;

        let events = self.events.clone();
        let requests = self.requests.clone();
        let hooks = self.hooks.clone();
        let mqtt_subs = self.mqtt_subs.clone();
        let mqtt_conns = self.mqtt_conns.clone();
        let stats = self.scheduler_stats.clone();
        sys.set(
            "status",
            self.lua
                .create_async_function(move |lua, target: Option<String>| {
                    let events = events.clone();
                    let requests = requests.clone();
                    let hooks = hooks.clone();
                    let mqtt_subs = mqtt_subs.clone();
                    let mqtt_conns = mqtt_conns.clone();
                    let stats = stats.clone();
                    let bus = bus.clone();
                    let own_name = own_name.clone();
                    async move {
                        let is_self = target.is_none() || target.as_deref() == Some(own_name.as_str());
                        if is_self {
                            let snap = build_snapshot_from_parts(
                                &events, &requests, &hooks, &mqtt_subs, &mqtt_conns, &stats,
                            );
                            return lua.to_value(&snap);
                        }
                        let name = target.unwrap();
                        match bus.find_by_name(&name) {
                            Some(handle) => {
                                let snap = handle
                                    .introspect()
                                    .await
                                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                                lua.to_value(&snap)
                            }
                            None => Err(mlua::Error::runtime(format!(
                                "目标模块 {} 不存在或未运行",
                                name
                            ))),
                        }
                    }
                })?,
        )?;

        globals.set("sys", sys)?;
        Ok(())
    }

    /// 引擎关闭前触发内置生命周期钩子（仅执行 `hook.on` 注册的处理器，无 payload、无返回值），
    /// 供脚本在被卸载/热重载或进程退出前做收尾（如保存最终状态、通知外部系统）。
    /// 出错只记警告，不影响正常关闭流程。
    async fn run_lifecycle_hook(&self, name: &str) {
        let funcs: Vec<mlua::Function> = {
            let binding = self.hooks.read().unwrap();
            binding
                .on
                .get(name)
                .map(|list| {
                    list.iter()
                        .filter_map(|k| self.lua.registry_value::<mlua::Function>(k).ok())
                        .collect()
                })
                .unwrap_or_default()
        };
        for func in funcs {
            self.budget.reset();
            if let Err(e) = func.call_async::<()>(()).await {
                tracing::warn!("[mod] 生命周期钩子 {} 执行出错: {}", name, e);
            }
        }
    }

    /// 引擎关闭时统一断开该脚本开出的所有 MQTT 连接，避免热更新/卸载脚本后连接和后台轮询任务残留
    async fn shutdown_mqtt_conns(&self) {
        let entries: Vec<ConnEntry> = std::mem::take(&mut *self.mqtt_conns.lock().unwrap());
        for entry in entries {
            let _ = tokio::time::timeout(Duration::from_secs(2), entry.client.disconnect()).await;
            entry.task.abort();
        }
    }

    async fn drain_dc_changes(&mut self) -> mod_engine::Result<()> {
        let mut msgs = vec![];
        while let Ok(msg) = self.dc_changed_rx.try_recv() {
            msgs.push(msg);
        }
        for (dev_id, snapshot) in msgs {
            self.emit_dc_changed(&dev_id, &snapshot).await?;
        }
        Ok(())
    }

    /// 异步运行直到收到 Shutdown 命令或 sender 全部 drop
    pub async fn run(mut self) -> mod_engine::Result<()> {
        loop {
            if self.drain_commands().await? {
                break;
            }

            self.drain_dc_changes().await?;

            self.scheduler.tick(&self.lua).await?;

            let sleep_dur = self
                .scheduler
                .next_wake()
                .map(|wake| {
                    wake.saturating_duration_since(tokio::time::Instant::now())
                        .min(Duration::from_millis(100))
                })
                .unwrap_or(Duration::from_millis(100));

            // sleep_dur 为零时直接 yield，避免 sleep(0) 不挂起任务导致单核 100%
            if sleep_dur.is_zero() {
                tokio::task::yield_now().await;
                continue;
            }

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
                changed = self.dc_changed_rx.recv() => {
                    if let Some((dev_id, snapshot)) = changed {
                        self.emit_dc_changed(&dev_id, &snapshot).await?;
                    }
                }
            }
        }
        tracing::info!("[mod] 引擎已关闭");
        Ok(())
    }
}
