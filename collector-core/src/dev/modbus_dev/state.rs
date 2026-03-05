use std::sync::atomic::{AtomicU8, Ordering};

use tracing::info;

use crate::dev::LifecycleState;

/// 加载当前状态
/// # 参数
/// - `state`: 状态原子变量
/// # 返回值
/// - `LifecycleState`: 当前状态
pub(super) fn load_state(state: &AtomicU8) -> LifecycleState {
    match state.load(Ordering::Acquire) {
        0 => LifecycleState::New,
        1 => LifecycleState::Initializing,
        2 => LifecycleState::Ready,
        3 => LifecycleState::Starting,
        4 => LifecycleState::Connecting,
        5 => LifecycleState::Connected,
        6 => LifecycleState::Running,
        7 => LifecycleState::Stopping,
        8 => LifecycleState::Stopped,
        9 => LifecycleState::Failed,
        _ => LifecycleState::Failed,
    }
}

/// 尝试更新状态
/// # 参数
/// - `state`: 状态原子变量
/// - `from`: 当前状态
/// - `to`: 目标状态
/// # 返回值
/// - `bool`: 是否成功更新状态
pub(super) fn cas_state(state: &AtomicU8, from: LifecycleState, to: LifecycleState) -> bool {
    state
        .compare_exchange(from as u8, to as u8, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

/// 存储状态
/// # 参数
/// - `id`: 设备ID
/// - `state`: 状态原子变量
/// - `to`: 目标状态
pub(super) fn store_state(id: &str, state: &AtomicU8, to: LifecycleState) {
    let from = load_state(state);
    state.store(to as u8, Ordering::Release);
    info!("[{}]{} -> {}", id, from, to);
}
