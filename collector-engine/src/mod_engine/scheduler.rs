use std::{collections::BinaryHeap, time::Duration};

use mlua::{Function, RegistryKey, Thread, ThreadStatus};
use tokio::time::Instant;

use crate::mod_engine::{
    errors::SchedulerError,
    timer_task::{CoroTask, TimerTask},
};

pub struct Scheduler {
    next_id: u64,
    timers: BinaryHeap<TimerTask>,
    coros: BinaryHeap<CoroTask>,
}

impl Scheduler {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            timers: BinaryHeap::new(),
            coros: BinaryHeap::new(),
        }
    }

    fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn add_after(&mut self, delay: Duration, callback: RegistryKey) {
        let task = TimerTask {
            id: self.alloc_id(),
            next_run: Instant::now() + delay,
            interval: None,
            callback,
        };
        self.timers.push(task);
    }

    pub fn add_every(&mut self, interval: Duration, callback: RegistryKey) {
        let task = TimerTask {
            id: self.alloc_id(),
            next_run: Instant::now() + interval,
            interval: Some(interval),
            callback,
        };
        self.timers.push(task);
    }

    /// 注册一个协程任务，立即（now）首次 resume
    pub fn add_coroutine(&mut self, thread: Thread) {
        let task = CoroTask {
            id: self.alloc_id(),
            wake_at: Instant::now(),
            thread,
        };
        self.coros.push(task);
    }

    /// 驱动一次调度：执行所有到期的回调任务和协程任务
    pub fn tick(&mut self, lua: &mlua::Lua) -> crate::mod_engine::Result<()> {
        let now = Instant::now();

        // 驱动回调定时器
        while let Some(task) = self.timers.peek() {
            if task.next_run > now {
                break;
            }
            let mut task = self.timers.pop().ok_or(SchedulerError::TaskNotFound)?;
            let func: Function = lua.registry_value(&task.callback)?;
            func.call::<()>(())?;
            if let Some(interval) = task.interval {
                task.next_run = Instant::now() + interval;
                self.timers.push(task);
            }
        }

        // 驱动协程
        while let Some(task) = self.coros.peek() {
            if task.wake_at > now {
                break;
            }
            let task = self.coros.pop().ok_or(SchedulerError::TaskNotFound)?;

            if task.thread.status() != ThreadStatus::Resumable {
                continue;
            }

            // resume 协程；若它调用了 wait(ms)，会 yield 出一个毫秒数
            let resume_result: mlua::Result<mlua::MultiValue> = task.thread.resume(());
            match resume_result {
                Ok(vals) => {
                    // 协程 yield 出来，取第一个返回值作为 sleep ms
                    if task.thread.status() == ThreadStatus::Resumable {
                        let ms = vals.iter().next().and_then(|v| v.as_u32()).unwrap_or(0);
                        let wake_at = Instant::now() + Duration::from_millis(ms as u64);
                        self.coros.push(CoroTask {
                            id: task.id,
                            wake_at,
                            thread: task.thread,
                        });
                    }
                    // 若协程已结束（return），直接丢弃
                }
                Err(e) => {
                    tracing::error!("[mod] 协程运行错误: {}", e);
                    // 错误的协程直接丢弃，不再重新入队
                }
            }
        }

        Ok(())
    }

    /// 返回距离下一个任务的等待时间，用于驱动层决定 sleep 多久
    pub fn next_wake(&self) -> Option<Instant> {
        let t = self.timers.peek().map(|t| t.next_run);
        let c = self.coros.peek().map(|c| c.wake_at);
        match (t, c) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }
}
