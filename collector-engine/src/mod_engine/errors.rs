#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Lua错误: {0}")]
    Lua(#[from] mlua::Error),
    #[error("Scheduler错误: {0}")]
    Scheduler(#[from] SchedulerError),
    #[error("引擎已关闭，无法发送命令")]
    EngineClosed,
    #[error("脚本加载失败: {0}")]
    ScriptLoad(String),
}

#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    #[error("timer task not found")]
    TaskNotFound,
}
