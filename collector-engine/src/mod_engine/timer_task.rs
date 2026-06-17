use std::time::Duration;

use mlua::RegistryKey;
use tokio::time::Instant;

/// 基于回调的定时任务（timer.after / timer.every）
#[derive(Debug)]
pub struct TimerTask {
    pub id: u64,
    pub next_run: Instant,
    pub interval: Option<Duration>,
    pub callback: RegistryKey,
}

impl PartialEq for TimerTask {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TimerTask {}

impl PartialOrd for TimerTask {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimerTask {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.next_run.cmp(&self.next_run)
    }
}

/// 协程任务（task.spawn 创建，wait 挂起后重新入队）
pub struct CoroTask {
    pub id: u64,
    pub wake_at: Instant,
    /// AsyncThread 作为 Stream，每次 next() 推进一步 yield
    pub stream: mlua::AsyncThread<mlua::MultiValue>,
}

impl PartialEq for CoroTask {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for CoroTask {}

impl PartialOrd for CoroTask {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CoroTask {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.wake_at.cmp(&self.wake_at)
    }
}
