use std::path::{Path, PathBuf};

use collector_core::center::SharedPointCenter;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

mod api;
mod loader;
mod scheduler;
mod watcher;

use loader::scan_dir;
use scheduler::Scheduler;
use watcher::{FileEvent, watch_dir};

pub struct ScriptEngine {
    script_dir: PathBuf,
    center: SharedPointCenter,
}

#[derive(thiserror::Error, Debug)]
pub enum ScriptEngineError {
    #[error("IO 错误: {0}")]
    Io(String),
    #[error("Lua 错误: {0}")]
    Lua(String),
    #[error("脚本缺少 TASK 表: {0}")]
    MissingTask(String),
    #[error("缺少必填字段: {0}")]
    MissingField(String),
    #[error("无效的 cron 表达式 '{0}': {1}")]
    InvalidCron(String, String),
    #[error("interval 必须大于 0")]
    InvalidInterval,
    #[error("文件监听错误: {0}")]
    Notify(String),
}

impl From<notify::Error> for ScriptEngineError {
    fn from(e: notify::Error) -> Self {
        ScriptEngineError::Notify(e.to_string())
    }
}

impl ScriptEngine {
    pub fn new(script_dir: impl AsRef<Path>, center: SharedPointCenter) -> Self {
        Self {
            script_dir: script_dir.as_ref().to_owned(),
            center,
        }
    }

    /// 启动引擎：扫描脚本目录、注册任务、启动热更新监听。
    /// `shutdown` 取消时引擎停止所有脚本任务并退出。
    pub async fn run(self, shutdown: CancellationToken) -> Result<(), ScriptEngineError> {
        // 确保目录存在
        tokio::fs::create_dir_all(&self.script_dir)
            .await
            .map_err(|e| ScriptEngineError::Io(e.to_string()))?;

        // 启动文件监听（watcher 必须持有到主循环结束）
        let (event_tx, event_rx) = mpsc::channel::<FileEvent>(64);
        let (watcher, mut notify_rx) = watch_dir(&self.script_dir)?;

        // 将 notify 事件桥接到 mpsc channel
        tokio::spawn(async move {
            let _watcher = watcher;
            while let Some(event) = notify_rx.recv().await {
                if event_tx.send(event).await.is_err() {
                    break;
                }
            }
        });

        let mut scheduler = Scheduler::new(self.center);

        // 初始扫描：加载已有脚本
        let scripts = scan_dir(&self.script_dir).await;
        tracing::info!("发现 {} 个脚本", scripts.len());
        for meta in scripts {
            scheduler.spawn(meta);
        }

        // 主循环：处理热更新事件，收到关闭信号时退出
        scheduler.run(event_rx, shutdown).await;

        tracing::info!("Lua 脚本引擎已停止");
        Ok(())
    }
}
