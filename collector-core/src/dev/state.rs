use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tracing::info;

use crate::dev::LifecycleState;

/// 设备生命周期状态的共享原子封装。
///
/// 用于在设备实例、后台任务等多个持有方之间安全共享状态，
/// 并统一状态读写和日志输出逻辑。
#[derive(Clone, Debug)]
pub(crate) struct SharedState(Arc<AtomicU8>);

impl SharedState {
    /// 使用给定的初始生命周期状态创建共享状态。
    pub(crate) fn new(initial: LifecycleState) -> Self {
        Self(Arc::new(AtomicU8::new(initial as u8)))
    }

    /// 读取当前生命周期状态。
    pub(crate) fn load(&self) -> LifecycleState {
        self.0.load(Ordering::Acquire).into()
    }

    /// 仅当当前状态等于 `from` 时，原子地更新为 `to`。
    ///
    /// 返回值表示这次状态迁移是否成功。
    pub(crate) fn cas(&self, from: LifecycleState, to: LifecycleState) -> bool {
        self.0
            .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    /// 直接写入目标状态，并记录状态迁移日志。
    pub(crate) fn store(&self, id: &str, to: LifecycleState) {
        let from = self.load();
        self.0.store(to as u8, Ordering::Release);
        info!("[{}]{} -> {}", id, from, to);
    }
}
