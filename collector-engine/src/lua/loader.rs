use std::path::{Path, PathBuf};

use mlua::{Lua, LuaOptions, StdLib};
use tokio::fs;

use crate::lua::ScriptEngineError;

/// 脚本调度方式
#[derive(Debug, Clone)]
pub enum Schedule {
    /// cron 表达式，如 "*/5 * * * * *"（6字段，含秒）
    Cron(String),
    /// 固定毫秒间隔
    Interval(u64),
}

/// 从 .lua 文件加载后解析出的脚本元信息
#[derive(Debug, Clone)]
pub struct ScriptMeta {
    pub path: PathBuf,
    pub name: String,
    pub schedule: Schedule,
    /// 脚本源码
    pub source: String,
}

/// 扫描目录，返回所有成功解析的脚本元信息
pub async fn scan_dir(dir: &Path) -> Vec<ScriptMeta> {
    let mut result = Vec::new();

    let mut read_dir = match fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(err) => {
            tracing::error!("扫描脚本目录失败: {}", err);
            return result;
        }
    };

    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lua") {
            continue;
        }
        // 跳过 _ 开头的文件（类型定义、模板等辅助文件）
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('_'))
        {
            continue;
        }
        match load_script(&path).await {
            Ok(meta) => {
                tracing::info!("加载脚本: {} ({})", meta.name, path.display());
                result.push(meta);
            }
            Err(err) => {
                tracing::warn!("跳过脚本 {}: {}", path.display(), err);
            }
        }
    }

    result
}

/// 加载单个 .lua 文件并解析 TASK 元信息
pub async fn load_script(path: &Path) -> Result<ScriptMeta, ScriptEngineError> {
    let source = fs::read_to_string(path)
        .await
        .map_err(|e| ScriptEngineError::Io(e.to_string()))?;

    let meta = parse_task_meta(path, &source)?;
    Ok(meta)
}

/// 在临时 Lua VM 中执行脚本，读取 TASK 全局表
fn parse_task_meta(path: &Path, source: &str) -> Result<ScriptMeta, ScriptEngineError> {
    // 只开启基础库，避免副作用
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::default())
        .map_err(|e| ScriptEngineError::Lua(e.to_string()))?;

    lua.load(source)
        .exec()
        .map_err(|e| ScriptEngineError::Lua(format!("{}: {}", path.display(), e)))?;

    let task: mlua::Table = lua
        .globals()
        .get("TASK")
        .map_err(|_| ScriptEngineError::MissingTask(path.display().to_string()))?;

    let name: String = task
        .get("name")
        .map_err(|_| ScriptEngineError::MissingField("TASK.name".into()))?;

    let schedule = if let Ok(cron_str) = task.get::<String>("schedule") {
        // 校验 cron 表达式合法性
        cron_str
            .parse::<cron::Schedule>()
            .map_err(|e| ScriptEngineError::InvalidCron(cron_str.clone(), e.to_string()))?;
        Schedule::Cron(cron_str)
    } else if let Ok(ms) = task.get::<u64>("interval") {
        if ms == 0 {
            return Err(ScriptEngineError::InvalidInterval);
        }
        Schedule::Interval(ms)
    } else {
        return Err(ScriptEngineError::MissingField(
            "TASK.schedule 或 TASK.interval".into(),
        ));
    };

    Ok(ScriptMeta {
        path: path.to_owned(),
        name,
        schedule,
        source: source.to_owned(),
    })
}
