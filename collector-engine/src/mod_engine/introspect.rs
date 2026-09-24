use serde::Serialize;

/// 一次 `sys.status()`/`ModEngineHandle::introspect()` 查询返回的单引擎运行时快照
#[derive(Serialize)]
pub struct EngineSnapshot {
    /// event.on 订阅的事件名 + 各自处理器数量
    pub events: Vec<EventInfo>,
    /// event.on_request 注册的请求名
    pub requests: Vec<String>,
    /// hook.on 注册的钩子名 + 处理器数量
    pub hooks_on: Vec<HookInfo>,
    /// hook.override 注册的钩子名
    pub hooks_override: Vec<String>,
    /// 待执行的 timer.after/every 数量
    pub timers: usize,
    /// 活跃的 task.spawn 协程数量
    pub coroutines: usize,
    /// 脚本自建的 mqtt 连接数
    pub mqtt_conns: usize,
    /// mqtt 订阅列表
    pub mqtt_subs: Vec<MqttSubInfo>,
}

#[derive(Serialize)]
pub struct EventInfo {
    pub name: String,
    pub handlers: usize,
}

#[derive(Serialize)]
pub struct HookInfo {
    pub name: String,
    pub handlers: usize,
}

#[derive(Serialize)]
pub struct MqttSubInfo {
    pub conn_id: u64,
    pub filter: String,
}

/// 脚本清单条目：`GlobalBus::list()` 和 `GlobalBus::snapshot_all()` 共用
#[derive(Serialize, Clone)]
pub struct ScriptInfo {
    pub name: String,
    pub path: String,
}

/// 一个脚本的完整自省结果：脚本身份 + 引擎内部快照，供 `GlobalBus::snapshot_all()` 使用
#[derive(Serialize)]
pub struct ScriptStatus {
    pub name: String,
    pub path: String,
    pub snapshot: EngineSnapshot,
}
