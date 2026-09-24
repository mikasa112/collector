use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use collector_core::{dev::can_bus::SharedCanBus, dock::mqtt::MqttOverrideStore};
use tokio_util::sync::CancellationToken;

use crate::mod_engine::{
    api::store::{LuaStore, new_store},
    engine::{ModEngine, ModEngineHandle},
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
}

impl ScriptInstance {
    async fn spawn(
        meta: &ScriptMeta,
        override_store: Option<MqttOverrideStore>,
        store: LuaStore,
        can_bus: Option<SharedCanBus>,
        script_dir: PathBuf,
    ) -> Option<Self> {
        let owned_topics: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (engine, handle) = match ModEngine::create(
            override_store.clone(),
            owned_topics.clone(),
            store,
            can_bus,
            script_dir,
        ) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("[mod:{}] 引擎创建失败: {}", meta.name, e);
                return None;
            }
        };

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
            return None;
        }

        Some(Self {
            handle,
            join,
            owned_topics,
            override_store,
        })
    }

    async fn shutdown(self) {
        self.handle.shutdown();
        let _ = self.join.await;
        if let Some(store) = self.override_store {
            let topics = self.owned_topics.lock().unwrap();
            store.clear_all(&topics);
        }
    }
}

pub struct ScriptManager {
    override_store: Option<MqttOverrideStore>,
    store: LuaStore,
    can_bus: Option<SharedCanBus>,
    /// 脚本目录，用于给各脚本 VM 配置 `require` 搜索路径
    script_dir: PathBuf,
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
            can_bus,
            script_dir: PathBuf::new(),
            scripts: HashMap::new(),
            last_reload: HashMap::new(),
            manifest: None,
        }
    }

    /// 该文件名是否允许执行；未启用注册总纲时（`manifest` 为 `None`）永远放行
    fn allows(&self, filename: &str) -> bool {
        match &self.manifest {
            None => true,
            Some(manifest) => manifest.is_enabled(filename),
        }
    }

    async fn load(&mut self, meta: ScriptMeta) {
        if let Some(old) = self.scripts.remove(&meta.path) {
            old.shutdown().await;
        }
        let path = meta.path.clone();
        let name = meta.name.clone();
        if let Some(instance) = ScriptInstance::spawn(
            &meta,
            self.override_store.clone(),
            self.store.clone(),
            self.can_bus.clone(),
            self.script_dir.clone(),
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
        for meta in metas {
            let filename = file_name_of(&meta.path);
            if self.allows(&filename) {
                self.load(meta).await;
            } else {
                tracing::info!("[mod] 跳过未注册模块: {} ({})", meta.name, meta.path.display());
            }
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
        for meta in metas {
            let filename = file_name_of(&meta.path);
            let running = self.scripts.contains_key(&meta.path);
            if self.allows(&filename) {
                if !running {
                    self.load(meta).await;
                }
            } else if running {
                tracing::info!("[mod] 模块未注册，卸载: {} ({})", meta.name, meta.path.display());
                self.unload(&meta.path).await;
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
                            self.load(meta).await;
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
