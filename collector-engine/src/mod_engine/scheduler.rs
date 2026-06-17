use std::{collections::BinaryHeap, time::Duration};

use futures_util::StreamExt;
use mlua::{Function, RegistryKey, Thread};
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
        let id = self.alloc_id();
        match thread.into_async::<mlua::MultiValue>(()) {
            Ok(stream) => {
                self.coros.push(CoroTask {
                    id,
                    wake_at: Instant::now(),
                    stream,
                });
            }
            Err(e) => {
                tracing::error!("[mod] 协程创建失败: {}", e);
            }
        }
    }

    /// 驱动一次调度：执行所有到期的回调任务和协程任务
    pub async fn tick(&mut self, lua: &mlua::Lua) -> crate::mod_engine::Result<()> {
        let now = Instant::now();

        // 驱动回调定时器
        while let Some(task) = self.timers.peek() {
            if task.next_run > now {
                break;
            }
            let mut task = self.timers.pop().ok_or(SchedulerError::TaskNotFound)?;
            let func: Function = lua.registry_value(&task.callback)?;
            func.call_async::<()>(()).await?;
            if let Some(interval) = task.interval {
                task.next_run = Instant::now() + interval;
                self.timers.push(task);
            }
        }

        // 驱动协程：取出所有到期任务，逐一推进一步
        let mut ready = Vec::new();
        while let Some(task) = self.coros.peek() {
            if task.wake_at > now {
                break;
            }
            ready.push(self.coros.pop().ok_or(SchedulerError::TaskNotFound)?);
        }

        for mut task in ready {
            match task.stream.next().await {
                Some(Ok(vals)) => {
                    // 协程 yield 出一个 ms 数，重新入队等待
                    // as_integer 仅匹配整数；浮点数需 as_f64 兜底，否则 wait(1000.0) 会导致 0ms 忙转
                    let ms = vals
                        .iter()
                        .next()
                        .and_then(|v| {
                            v.as_integer()
                                .map(|i| i.max(0) as u64)
                                .or_else(|| v.as_f64().map(|f| f.max(0.0) as u64))
                        })
                        .unwrap_or(0);
                    task.wake_at = Instant::now() + Duration::from_millis(ms as u64);
                    self.coros.push(task);
                }
                Some(Err(e)) => {
                    tracing::error!("[mod] 协程运行错误: {}", e);
                    // 出错直接丢弃
                }
                None => {
                    // 协程已结束，丢弃
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
