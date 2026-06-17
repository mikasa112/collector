pub mod client;

use std::sync::Arc;

use dashmap::DashMap;

/// topic → 覆盖 payload（None 表示已清除，回到原始采集值）
#[derive(Clone, Default)]
pub struct MqttOverrideStore(Arc<DashMap<String, serde_json::Value>>);

impl MqttOverrideStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 覆盖 topic 的推送内容
    pub fn set(&self, topic: impl Into<String>, value: serde_json::Value) {
        self.0.insert(topic.into(), value);
    }

    /// 取消覆盖，恢复原始采集值
    pub fn clear(&self, topic: &str) {
        self.0.remove(topic);
    }

    /// 批量取消覆盖
    pub fn clear_all(&self, topics: &[String]) {
        for topic in topics {
            self.0.remove(topic.as_str());
        }
    }

    /// 查询某个 topic 的覆盖值
    pub fn get(&self, topic: &str) -> Option<serde_json::Value> {
        self.0.get(topic).map(|v| v.clone())
    }
}
