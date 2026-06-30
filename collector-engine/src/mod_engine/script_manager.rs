use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use collector_core::{center::SharedPointCenter, dock::mqtt::MqttOverrideStore};
use tokio_util::sync::CancellationToken;

use crate::mod_engine::{
    api::store::{LuaStore, new_store},
    engine::{ModEngine, ModEngineHandle},
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
        center: SharedPointCenter,
        override_store: Option<MqttOverrideStore>,
        store: LuaStore,
    ) -> Option<Self> {
        let owned_topics: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (engine, handle) =
            match ModEngine::create(center, override_store.clone(), owned_topics.clone(), store) {
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
    center: SharedPointCenter,
    override_store: Option<MqttOverrideStore>,
    store: LuaStore,
    scripts: HashMap<PathBuf, ScriptInstance>,
    /// 记录每个路径最近一次处理时间，用于热更新去抖
    last_reload: HashMap<PathBuf, Instant>,
}

impl ScriptManager {
    pub fn new(center: SharedPointCenter, override_store: Option<MqttOverrideStore>) -> Self {
        Self {
            center,
            override_store,
            store: new_store(),
            scripts: HashMap::new(),
            last_reload: HashMap::new(),
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
            self.center.clone(),
            self.override_store.clone(),
            self.store.clone(),
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

        // 先启动 watcher，再扫描，避免扫描期间的文件变化事件丢失
        let (watcher, mut notify_rx) = watch_dir(&script_dir)
            .map_err(|e| crate::mod_engine::script_loader::LoadError::Io(e.to_string()))?;
        let _watcher = watcher;

        // 初始扫描
        let metas = script_loader::scan_dir(&script_dir).await;
        tracing::info!("[mod] 初始加载 {} 个脚本", metas.len());
        for meta in metas {
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

                tracing::info!("[mod] 热更新: {}", path.display());
                match script_loader::load_script(&path).await {
                    Ok(meta) => self.load(meta).await,
                    Err(e) => tracing::warn!("[mod] 热更新失败 {}: {}", path.display(), e),
                }
            }
            FileEvent::Remove(path) => {
                self.last_reload.remove(&path);
                self.unload(&path).await;
            }
        }
    }
}
