use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// 单次"从 Rust 调入 Lua"的执行时间预算：超过该时长仍未返回或让出（wait() 挂起也算让出），
/// 下一次全局指令钩子检查点会中断当前调用并报错，防止脚本死循环卡死整个引擎。
const SCRIPT_BUDGET: Duration = Duration::from_millis(200);

/// 全局指令钩子每隔多少条 VM 字节码指令检查一次预算：越小越及时但检查开销越高
pub const HOOK_INSTRUCTION_COUNT: u32 = 10_000;

#[derive(Clone)]
pub struct Budget(Arc<Mutex<Instant>>);

impl Budget {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Instant::now())))
    }

    /// 在每次即将调用 Lua 函数前调用：重置为"从现在起 SCRIPT_BUDGET 时长内必须完成"
    pub fn reset(&self) {
        *self.0.lock().unwrap() = Instant::now() + SCRIPT_BUDGET;
    }

    /// 供全局指令钩子调用：当前是否已超出预算
    pub fn is_expired(&self) -> bool {
        Instant::now() > *self.0.lock().unwrap()
    }
}
