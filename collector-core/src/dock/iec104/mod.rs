mod server;

use std::sync::atomic::{AtomicI32, Ordering};

pub use server::{Iec104Server, Iec104ServerError};

/// 北向子站当前已建立的主站连接数；子站未运行时为 -1。
static CONNECTED_MASTERS: AtomicI32 = AtomicI32::new(-1);

/// 北向子站当前的主站连接数；`None` 表示北向子站没有运行。
pub fn connected_masters() -> Option<u32> {
    u32::try_from(CONNECTED_MASTERS.load(Ordering::Relaxed)).ok()
}

pub(crate) fn set_connected_masters(n: i32) {
    CONNECTED_MASTERS.store(n, Ordering::Relaxed);
}
