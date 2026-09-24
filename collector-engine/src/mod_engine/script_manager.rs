use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use collector_core::{dev::can_bus::SharedCanBus, dock::mqtt::MqttOverrideStore};
use tokio_util::sync::CancellationToken;

use crate::mod_engine::{
    api::store::{LuaStore, new_store},
    engine::{ModEngine, ModEngineHandle},
    global_bus::GlobalBus,
    manifest::{MANIFEST_FILE, Manifest},
    script_loader::{self, ScriptMeta},
    watcher::{FileEvent, watch_dir},
};

/// 同一路径两次 Upsert 事件之间的最小间隔，小于此值的重复事件被忽略
const DEBOUNCE: Duration = Duration::from_millis(200);

struct ScriptInstance {
    handle: ModEngineHandle,
    join: tokio::task::JoinHandle<()>,
    owned_topics: Arc<Mutex<Vec<String>>>,
    override_store: Option<MqttOverrideStore>,
    bus: GlobalBus,
    /// 在 `bus` 中登记时使用的 key（脚本路径字符串），卸载时用于反注册
    key: String,
}

impl ScriptInstance {
    async fn spawn(
        meta: &ScriptMeta,
        override_store: Option<MqttOverrideStore>,
        store: LuaStore,
        can_bus: Option<SharedCanBus>,
        script_dir: PathBuf,
        data_file: PathBuf,
        bus: GlobalBus,
    ) -> Option<Self> {
        let owned_topics: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let key = meta.path.to_string_lossy().to_string();
        let (engine, handle) = match ModEngine::create(
            override_store.clone(),
            owned_topics.clone(),
            store,
            can_bus,
            script_dir,
            data_file,
            bus.clone(),
            meta.name.clone(),
        ) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("[mod:{}] 引擎创建失败: {}", meta.name, e);
                return None;
            }
        };

        bus.register(key.clone(), meta.name.clone(), handle.clone());

        let name = meta.name.clone();
        let join = tokio::spawn(async move {
            if let Err(e) = engine.run().await {
                tracing::error!("[mod:{}] 引擎运行错误: {}", name, e);
            }
        });

        if let Err(e) = handle.load_script(&meta.source).await {
            tracing::error!("[mod:{}] {}", meta.name, e);
            handle.shutdown();
            let _ = join.await;
            bus.unregister(&key);
            return None;
        }

        Some(Self {
            handle,
            join,
            owned_topics,
            override_store,
            bus,
            key,
        })
    }

    async fn shutdown(self) {
        self.handle.shutdown();
        let _ = self.join.await;
        self.bus.unregister(&self.key);
        if let Some(store) = self.override_store {
            let topics = self.owned_topics.lock().unwrap();
            store.clear_all(&topics);
        }
    }
}

pub struct ScriptManager {
    override_store: Option<MqttOverrideStore>,
    store: LuaStore,
    /// 跨顶层脚本（跨 VM）事件广播表，供 `event.emit` 使用
    bus: GlobalBus,
    can_bus: Option<SharedCanBus>,
    /// 脚本目录，用于给各脚本 VM 配置 `require` 搜索路径
    script_dir: PathBuf,
    /// 持久化存档目录（`script_dir/.data`），按脚本文件名隔离存档文件
    save_dir: PathBuf,
    scripts: HashMap<PathBuf, ScriptInstance>,
    /// 记录每个路径最近一次处理时间，用于热更新去抖
    last_reload: HashMap<PathBuf, Instant>,
    /// 模块注册总纲：`None` 表示脚本目录下未放置 `_manifest.lua`，不启用注册限制（兼容旧行为，目录下脚本全部允许执行）；
    /// `Some` 表示已启用注册限制，只有总纲中列出的文件名才会被加载执行
    manifest: Option<Manifest>,
}

impl ScriptManager {
    pub fn new(override_store: Option<MqttOverrideStore>, can_bus: Option<SharedCanBus>) -> Self {
        Self {
            override_store,
            store: new_store(),
            bus: GlobalBus::new(),
            can_bus,
            script_dir: PathBuf::new(),
            save_dir: PathBuf::new(),
            scripts: HashMap::new(),
            last_reload: HashMap::new(),
            manifest: None,
        }
    }

    /// 拿到一个存活的、可随时查询的 `GlobalBus` handle，供 Rust 侧（future HTTP/CLI 层）
    /// 在 `run(self, ...)` 消费掉 `self` 之前保留下来做运行时自省查询
    pub fn bus(&self) -> GlobalBus {
        self.bus.clone()
    }

    /// 该文件名是否允许执行；未启用注册总纲时（`manifest` 为 `None`）永远放行
    fn allows(&self, filename: &str) -> bool {
        match &self.manifest {
            None => true,
            Some(manifest) => manifest.is_enabled(filename),
        }
    }

    /// 该文件名对应的脚本当前是否正在运行
    fn is_running_filename(&self, filename: &str) -> bool {
        self.scripts.keys().any(|p| file_name_of(p) == filename)
    }

    async fn load(&mut self, meta: ScriptMeta) {
        if let Some(old) = self.scripts.remove(&meta.path) {
            old.shutdown().await;
        }
        let path = meta.path.clone();
        let name = meta.name.clone();
        let stem = meta
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&name)
            .to_string();
        let data_file = self.save_dir.join(format!("{stem}.json"));
        if let Some(instance) = ScriptInstance::spawn(
            &meta,
            self.override_store.clone(),
            self.store.clone(),
            self.can_bus.clone(),
            self.script_dir.clone(),
            data_file,
            self.bus.clone(),
        )
        .await
        {
            tracing::info!("[mod:{}] 已启动 ({})", name, path.display());
            self.scripts.insert(path, instance);
        }
    }

    async fn unload(&mut self, path: &PathBuf) {
        if let Some(instance) = self.scripts.remove(path) {
            instance.shutdown().await;
            tracing::info!("[mod] 已卸载: {}", path.display());
        }
    }

    /// 扫描目录、启动所有脚本、监听热更新，直到 shutdown 信号
    pub async fn run(
        mut self,
        script_dir: impl AsRef<std::path::Path>,
        shutdown: CancellationToken,
    ) -> Result<(), crate::mod_engine::script_loader::LoadError> {
        let script_dir = script_dir.as_ref();

        tokio::fs::create_dir_all(script_dir)
            .await
            .map_err(|e| crate::mod_engine::script_loader::LoadError::Io(e.to_string()))?;

        // 规范化为绝对路径，保证 scan_dir 和 watcher 使用同一路径格式
        let script_dir = script_dir
            .canonicalize()
            .map_err(|e| crate::mod_engine::script_loader::LoadError::Io(e.to_string()))?;
        self.script_dir = script_dir.clone();
        self.save_dir = script_dir.join(".data");
        tokio::fs::create_dir_all(&self.save_dir)
            .await
            .map_err(|e| crate::mod_engine::script_loader::LoadError::Io(e.to_string()))?;

        // 先启动 watcher，再扫描，避免扫描期间的文件变化事件丢失
        let (watcher, mut notify_rx) = watch_dir(&script_dir)
            .map_err(|e| crate::mod_engine::script_loader::LoadError::Io(e.to_string()))?;
        let _watcher = watcher;

        self.manifest = Manifest::load(&script_dir).await;
        if self.manifest.is_some() {
            tracing::info!("[mod] 已启用模块注册总纲 {}，仅注册模块可执行", MANIFEST_FILE);
        } else {
            tracing::info!(
                "[mod] 未找到模块注册总纲 {}，跳过注册限制（兼容模式，目录下脚本全部允许执行）",
                MANIFEST_FILE
            );
        }

        // 初始扫描
        let metas = script_loader::scan_dir(&script_dir).await;
        tracing::info!("[mod] 初始加载 {} 个脚本", metas.len());
        let allowed: Vec<ScriptMeta> = metas
            .into_iter()
            .filter(|meta| {
                let filename = file_name_of(&meta.path);
                if self.allows(&filename) {
                    true
                } else {
                    tracing::info!("[mod] 跳过未注册模块: {} ({})", meta.name, meta.path.display());
                    false
                }
            })
            .collect();
        let (ordered, skipped) = resolve_load_order(allowed);
        for (filename, path, reason) in skipped {
            tracing::warn!("[mod] 跳过模块 {} ({}): {}", filename, path.display(), reason);
        }
        for meta in ordered {
            self.load(meta).await;
        }

        loop {
            tokio::select! {
                Some(event) = notify_rx.recv() => {
                    self.handle_file_event(event).await;
                }
                _ = shutdown.cancelled() => {
                    break;
                }
            }
        }

        // 关闭所有脚本实例，等待各引擎线程退出
        tracing::info!("[mod] 正在关闭所有脚本...");
        for (_, instance) in self.scripts.drain() {
            instance.shutdown().await;
        }
        tracing::info!("[mod] 脚本管理器已停止");
        Ok(())
    }

    /// 重新扫描脚本目录，按当前注册总纲状态对齐运行中的实例：
    /// 已注册且未运行的脚本启动，已运行但不再注册的脚本卸载。
    /// 用于总纲文件本身发生变化时（新增/删除/编辑）批量生效。
    async fn reconcile(&mut self) {
        let metas = script_loader::scan_dir(&self.script_dir).await;
        let mut allowed = Vec::new();
        for meta in metas {
            let filename = file_name_of(&meta.path);
            if self.allows(&filename) {
                allowed.push(meta);
            } else if self.scripts.contains_key(&meta.path) {
                tracing::info!("[mod] 模块未注册，卸载: {} ({})", meta.name, meta.path.display());
                self.unload(&meta.path).await;
            }
        }

        let (ordered, skipped) = resolve_load_order(allowed);
        for (filename, path, reason) in skipped {
            if self.scripts.contains_key(&path) {
                tracing::warn!("[mod] 模块 {} 依赖不再满足({})，卸载", filename, reason);
                self.unload(&path).await;
            } else {
                tracing::warn!("[mod] 跳过模块 {}: {}", filename, reason);
            }
        }

        for meta in ordered {
            if !self.scripts.contains_key(&meta.path) {
                self.load(meta).await;
            }
        }
    }

    async fn handle_file_event(&mut self, event: FileEvent) {
        match event {
            FileEvent::Upsert(path) => {
                // 去抖：同一路径 DEBOUNCE 时间内的重复事件忽略
                let now = Instant::now();
                if let Some(&last) = self.last_reload.get(&path)
                    && now.duration_since(last) < DEBOUNCE
                {
                    return;
                }
                self.last_reload.insert(path.clone(), now);

                if file_name_of(&path) == MANIFEST_FILE {
                    tracing::info!("[mod] 模块注册总纲变更，重新校验已加载脚本: {}", path.display());
                    self.manifest = Manifest::load(&self.script_dir).await;
                    self.reconcile().await;
                    return;
                }

                tracing::info!("[mod] 热更新: {}", path.display());
                match script_loader::load_script(&path).await {
                    Ok(meta) => {
                        let filename = file_name_of(&meta.path);
                        if self.allows(&filename) {
                            let missing: Vec<&String> = meta
                                .depends
                                .iter()
                                .filter(|d| !self.is_running_filename(d))
                                .collect();
                            if missing.is_empty() {
                                self.load(meta).await;
                            } else {
                                tracing::warn!(
                                    "[mod] 模块 {} 依赖不满足(缺少运行中的: {})，跳过加载",
                                    meta.name,
                                    missing
                                        .iter()
                                        .map(|s| s.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                );
                                self.unload(&meta.path).await;
                            }
                        } else {
                            tracing::info!(
                                "[mod] 跳过未注册模块: {} ({})",
                                meta.name,
                                meta.path.display()
                            );
                            // 该脚本可能此前已在运行（比如刚被从注册表移除后又编辑了文件），
                            // 确保它不会继续挂着旧实例
                            self.unload(&meta.path).await;
                        }
                    }
                    Err(e) => {
                        // 重命名/移动会被 notify 合并为一个事件，其中旧路径仍带 .lua
                        // 后缀，被误判为 Upsert；此时文件已不存在，应视为卸载，
                        // 否则旧脚本实例会永远残留在 self.scripts 中
                        if !tokio::fs::try_exists(&path).await.unwrap_or(true) {
                            tracing::info!("[mod] 文件已不存在，视为卸载: {}", path.display());
                            self.unload(&path).await;
                        } else {
                            tracing::warn!("[mod] 热更新失败 {}: {}", path.display(), e);
                        }
                    }
                }
            }
            FileEvent::Remove(path) => {
                self.last_reload.remove(&path);
                if file_name_of(&path) == MANIFEST_FILE {
                    tracing::warn!(
                        "[mod] 模块注册总纲已删除，回退为兼容模式（不再限制注册，目录下脚本全部允许执行）"
                    );
                    self.manifest = None;
                    self.reconcile().await;
                    return;
                }
                self.unload(&path).await;
            }
        }
    }
}

/// 提取路径的文件名部分（不含目录），用于和总纲条目 / `MANIFEST_FILE` 比较
fn file_name_of(path: &std::path::Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

/// 对已通过 `allows()` 过滤的脚本列表按 `MOD.depends` 依赖关系做拓扑排序（Kahn 算法）。
///
/// 返回按依赖顺序可加载的脚本列表，以及因依赖缺失或成环被跳过的 (文件名, 路径, 原因)。
/// 依赖目标不在传入列表中（未注册或不存在）视为缺失依赖；排序后仍有残留则视为依赖成环。
fn resolve_load_order(metas: Vec<ScriptMeta>) -> (Vec<ScriptMeta>, Vec<(String, PathBuf, String)>) {
    let mut by_filename: HashMap<String, ScriptMeta> = metas
        .into_iter()
        .map(|m| (file_name_of(&m.path), m))
        .collect();
    let available: HashSet<String> = by_filename.keys().cloned().collect();

    let mut skipped: Vec<(String, PathBuf, String)> = Vec::new();
    let mut skipped_names: HashSet<String> = HashSet::new();
    let mut in_degree: HashMap<String, usize> = HashMap::new();
    let mut dependents: HashMap<String, Vec<String>> = HashMap::new();

    for (filename, meta) in &by_filename {
        let missing: Vec<String> = meta
            .depends
            .iter()
            .filter(|d| !available.contains(*d))
            .cloned()
            .collect();
        if !missing.is_empty() {
            skipped.push((
                filename.clone(),
                meta.path.clone(),
                format!("缺少依赖: {}", missing.join(", ")),
            ));
            skipped_names.insert(filename.clone());
            continue;
        }
        in_degree.entry(filename.clone()).or_insert(0);
        for dep in &meta.depends {
            *in_degree.entry(filename.clone()).or_insert(0) += 1;
            dependents.entry(dep.clone()).or_default().push(filename.clone());
        }
    }

    let mut queue: VecDeque<String> = in_degree
        .iter()
        .filter(|&(name, &deg)| deg == 0 && !skipped_names.contains(name))
        .map(|(name, _)| name.clone())
        .collect();

    let mut order: Vec<String> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    while let Some(name) = queue.pop_front() {
        if !visited.insert(name.clone()) {
            continue;
        }
        order.push(name.clone());
        if let Some(deps) = dependents.get(&name) {
            for dependent in deps {
                if skipped_names.contains(dependent) {
                    continue;
                }
                if let Some(deg) = in_degree.get_mut(dependent) {
                    *deg -= 1;
                    if *deg == 0 {
                        queue.push_back(dependent.clone());
                    }
                }
            }
        }
    }

    for name in in_degree.keys() {
        if visited.contains(name) || skipped_names.contains(name) {
            continue;
        }
        if let Some(meta) = by_filename.get(name) {
            skipped.push((name.clone(), meta.path.clone(), "依赖出现循环".to_string()));
        }
    }

    let sorted_metas: Vec<ScriptMeta> = order
        .into_iter()
        .filter_map(|name| by_filename.remove(&name))
        .collect();

    (sorted_metas, skipped)
}
