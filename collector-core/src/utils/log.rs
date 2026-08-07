use std::collections::VecDeque;

use chrono::{DateTime, Local};
use parking_lot::Mutex;
use tokio::sync::broadcast;

#[derive(Clone)]
pub struct OperationLog {
    pub time: DateTime<Local>,
    pub level: OpLogLevel,
    pub message: String,
}

#[derive(Clone, Copy)]
pub enum OpLogLevel {
    Info,
    Warn,
    Error,
}

pub static OP_LOGGER: std::sync::LazyLock<OperationLogger> =
    std::sync::LazyLock::new(|| OperationLogger::new(30));

pub struct OperationLogger {
    logs: Mutex<VecDeque<OperationLog>>,
    tx: broadcast::Sender<OperationLog>,
    capacity: usize,
}

impl OperationLogger {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel::<OperationLog>(8);
        Self {
            logs: Mutex::new(VecDeque::new()),
            tx,
            capacity,
        }
    }

    pub fn push(&self, level: OpLogLevel, message: impl Into<String>) {
        let log = OperationLog {
            time: Local::now(),
            level,
            message: message.into(),
        };
        {
            let mut logs = self.logs.lock();
            if logs.len() >= self.capacity {
                logs.pop_front();
            }
            logs.push_back(log.clone());
        }
        let _ = self.tx.send(log);
    }

    pub fn all(&self) -> Vec<OperationLog> {
        self.logs.lock().iter().cloned().collect()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<OperationLog> {
        self.tx.subscribe()
    }

    pub fn clear(&self) {
        self.logs.lock().clear();
    }
}
