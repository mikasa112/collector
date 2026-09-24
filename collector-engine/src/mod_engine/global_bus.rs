use std::sync::{Arc, Mutex};

use crate::mod_engine::{
    engine::ModEngineHandle,
    introspect::{ScriptInfo, ScriptStatus},
};

/// 跨顶层脚本（跨 VM）事件广播表：key 为脚本路径字符串，value 为该脚本 VM 的命令句柄。
/// `broadcast` 对所有登记的句柄调用已有的 `ModEngineHandle::emit`，把事件投递进各自 VM 的
/// `EngineCmd::Emit` 队列，最终由各 VM 本地已有的 `event.on` 处理器接收——本模块只负责跨 VM 转发，
/// 不重新实现分发逻辑。`name` 是脚本声明的 `MOD.name`，供 `event.request` 按名字寻址目标脚本。
struct Entry {
    key: String,
    name: String,
    handle: ModEngineHandle,
}

#[derive(Clone, Default)]
pub struct GlobalBus(Arc<Mutex<Vec<Entry>>>);

impl GlobalBus {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, key: String, name: String, handle: ModEngineHandle) {
        self.0.lock().unwrap().push(Entry { key, name, handle });
    }

    pub fn unregister(&self, key: &str) {
        self.0.lock().unwrap().retain(|e| e.key != key);
    }

    /// 广播给所有仍在运行的顶层脚本（包括发出者自身）
    pub fn broadcast(&self, name: &str, value: serde_json::Value) {
        let handles: Vec<ModEngineHandle> =
            self.0.lock().unwrap().iter().map(|e| e.handle.clone()).collect();
        for h in handles {
            let _ = h.emit(name.to_owned(), value.clone());
        }
    }

    /// 按脚本声明的 `MOD.name` 寻址，供 `event.request` 定向发送。同名冲突时返回第一个匹配。
    pub fn find_by_name(&self, name: &str) -> Option<ModEngineHandle> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.handle.clone())
    }

    /// 列出当前所有运行中的顶层脚本（名称 + 路径），供 `sys.list_scripts()` 和运维查询使用
    pub fn list(&self) -> Vec<ScriptInfo> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|e| ScriptInfo {
                name: e.name.clone(),
                path: e.key.clone(),
            })
            .collect()
    }

    /// 依次向所有运行中的脚本发起自省查询并汇总结果，供 Rust 侧（`ScriptManager::bus()`）批量查询
    pub async fn snapshot_all(&self) -> Vec<ScriptStatus> {
        let entries: Vec<(String, String, ModEngineHandle)> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|e| (e.name.clone(), e.key.clone(), e.handle.clone()))
            .collect();
        let mut out = Vec::with_capacity(entries.len());
        for (name, path, handle) in entries {
            if let Ok(snapshot) = handle.introspect().await {
                out.push(ScriptStatus { name, path, snapshot });
            }
        }
        out
    }
}
