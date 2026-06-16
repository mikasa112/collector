use std::path::{Path, PathBuf};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;

/// 文件变化事件
#[derive(Debug, Clone)]
pub enum FileEvent {
    /// 文件创建或修改
    Upsert(PathBuf),
    /// 文件删除
    Remove(PathBuf),
}

/// 启动目录监听器，返回事件接收端。
///
/// 内部创建 `RecommendedWatcher`，通过同步 channel 桥接到 Tokio mpsc。
/// watcher 实例的所有权通过返回值传递给调用方，调用方持有它才能保持监听活跃。
pub fn watch_dir(
    dir: &Path,
) -> Result<(RecommendedWatcher, mpsc::Receiver<FileEvent>), notify::Error> {
    let (tx, rx) = mpsc::channel::<FileEvent>(64);

    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            let event = match res {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!("文件监听错误: {}", err);
                    return;
                }
            };

            for path in event.paths {
                if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                    continue;
                }

                let file_event = match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => FileEvent::Upsert(path),
                    EventKind::Remove(_) => FileEvent::Remove(path),
                    _ => continue,
                };

                if tx.blocking_send(file_event).is_err() {
                    // 接收端已关闭，退出监听
                    break;
                }
            }
        },
        Config::default(),
    )?;

    watcher.watch(dir, RecursiveMode::NonRecursive)?;

    Ok((watcher, rx))
}
