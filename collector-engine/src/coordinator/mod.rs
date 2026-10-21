//! 协调控制器（AGC/AVC）：把多台下级控制器合并成一个虚拟设备。
//! 说明见 `docs/coordinator.md`。

pub mod alloc;
pub mod core;
pub mod logic;

pub use core::{Coordinator, CoordinatorError};
