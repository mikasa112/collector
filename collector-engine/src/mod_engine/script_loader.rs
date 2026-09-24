use std::path::{Path, PathBuf};

use mlua::{Lua, LuaOptions, StdLib};
use tokio::fs;

/// 引擎当前的模组 API 版本。脚本可选择在 `MOD.api_version` 中声明其兼容的版本号：
/// 缺省（未声明）视为兼容，兼容旧脚本；声明但与此不相等则拒绝加载。
pub const ENGINE_API_VERSION: u32 = 1;

#[derive(Debug, Clone)]
pub struct ScriptMeta {
    pub path: PathBuf,
    pub name: String,
    pub description: String,
    pub source: String,
    /// 该脚本依赖的其它顶层脚本文件名（不含目录），要求依赖脚本已注册且运行中才允许加载
    pub depends: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("IO 错误: {0}")]
    Io(String),
    #[error("Lua 解析错误: {0}")]
    Lua(String),
    #[error("脚本缺少 MOD 表: {0}")]
    MissingMod(String),
    #[error("MOD.name 字段缺失或类型错误: {0}")]
    MissingName(String),
    #[error("模组 API 版本不兼容: {0}")]
    IncompatibleApiVersion(String),
}

/// 扫描目录，返回所有成功解析的脚本元信息（跳过 _ 开头的辅助文件）
pub async fn scan_dir(dir: &Path) -> Vec<ScriptMeta> {
    let mut result = Vec::new();
    let mut read_dir = match fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) => {
            tracing::error!("[mod] 扫描脚本目录失败: {}", e);
            return result;
        }
    };
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let path = match entry.path().canonicalize() {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("[mod] 路径规范化失败 {}: {}", entry.path().display(), e);
                continue;
            }
        };
        if path.extension().and_then(|e| e.to_str()) != Some("lua") {
            continue;
        }
        if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with('_'))
        {
            continue;
        }
        match load_script(&path).await {
            Ok(meta) => {
                tracing::info!("[mod] 发现脚本: {} ({})", meta.name, path.display());
                result.push(meta);
            }
            Err(e) => {
                tracing::warn!("[mod] 跳过脚本 {}: {}", path.display(), e);
            }
        }
    }
    result
}

/// 读取单个 .lua 文件并解析 MOD 元信息
pub async fn load_script(path: &Path) -> Result<ScriptMeta, LoadError> {
    let source = fs::read_to_string(path)
        .await
        .map_err(|e| LoadError::Io(e.to_string()))?;
    let meta = parse_mod_meta(path, &source)?;
    Ok(meta)
}

/// 在沙箱 VM 中执行脚本，读取 MOD 全局表
///
/// 脚本顶层可能调用 task/event/dc/log 等引擎 API。
/// 沙箱里通过 __index 把所有未知全局代理为空函数，
/// 使 API 调用静默忽略，只让 MOD = {...} 赋值生效。
fn parse_mod_meta(path: &Path, source: &str) -> Result<ScriptMeta, LoadError> {
    let lua = Lua::new_with(StdLib::NONE, LuaOptions::default())
        .map_err(|e| LoadError::Lua(e.to_string()))?;

    // 把全局环境的 __index 设为返回空函数的代理，
    // 让 task.spawn(fn) / event.on(...) 等调用变成无操作。
    // __call 返回空表而非 _proxy 本身，避免脚本对返回值做 ipairs/pairs
    // 迭代时因 __index 永远非 nil 而陷入死循环。
    lua.load(
        r#"
        local _proxy
        _proxy = setmetatable({}, {
            __index = function(_, _) return _proxy end,
            __call  = function(_, ...) return {} end,
        })
        setmetatable(_ENV, { __index = function(_, _) return _proxy end })
    "#,
    )
    .exec()
    .map_err(|e| LoadError::Lua(e.to_string()))?;

    if let Err(e) = lua.load(source).exec() {
        // 脚本顶层可能出现 `local m = require("xxx"); m.start(...)` 这类调用链，
        // 代理表在 __call 处返回的是普通空表（而非代理本身，见上方说明），
        // 因此链式调用会在沙箱里报错。只要 MOD 已在报错前赋值成功，
        // 元信息提取的目的已经达成，不应因脚本后续逻辑而整体判定为失败。
        if lua.globals().get::<mlua::Table>("MOD").is_err() {
            return Err(LoadError::Lua(format!("{}: {}", path.display(), e)));
        }
        tracing::warn!(
            "[mod] 脚本 {} 顶层执行未完全成功（MOD 元信息已提取，忽略后续错误）: {}",
            path.display(),
            e
        );
    }

    let mod_table: mlua::Table = lua
        .globals()
        .get("MOD")
        .map_err(|_| LoadError::MissingMod(path.display().to_string()))?;

    let name: String = mod_table
        .get("name")
        .map_err(|_| LoadError::MissingName(path.display().to_string()))?;

    let description: String = mod_table.get("description").unwrap_or_default();

    let api_version: Option<u32> = mod_table.get("api_version").unwrap_or(None);
    if let Some(v) = api_version
        && v != ENGINE_API_VERSION
    {
        return Err(LoadError::IncompatibleApiVersion(format!(
            "{}: 脚本声明 api_version={}，引擎当前 api_version={}",
            path.display(),
            v,
            ENGINE_API_VERSION
        )));
    }

    let depends: Vec<String> = mod_table.get("depends").unwrap_or_default();

    Ok(ScriptMeta {
        path: path.to_owned(),
        name,
        description,
        source: source.to_owned(),
        depends,
    })
}
