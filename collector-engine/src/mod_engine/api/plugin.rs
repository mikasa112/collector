use std::path::{Path, PathBuf};

use mlua::{Lua, Table};

/// 提供顶层脚本用于自动发现子目录下可 `require` 的模块（如 `plugins/` 目录下的插件），
/// 使插件的增删不必手动编辑调用脚本本身。
///
/// 只做"加载时扫描"，不支持热重载：模块被 require 进调用脚本所在的 VM 后即固定运行，
/// 若要令新增/修改的插件生效，需要重新触发整个顶层脚本的加载（如 touch 该顶层脚本文件）。
pub fn create_plugin_table(lua: &Lua, script_dir: PathBuf) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set(
        "list",
        lua.create_function(move |_, subdir: String| Ok(list_modules(&script_dir, &subdir)))?,
    )?;
    Ok(table)
}

/// 扫描 `script_dir/subdir` 下的 `.lua` 文件（跳过 `_` 开头的文件，与其它模块扫描一致，
/// 可借此约定给某个插件文件加上 `_` 前缀来临时禁用它），返回形如 "subdir.filename" 的
/// 模块路径（按文件名排序），可直接传给 `require`。
fn list_modules(script_dir: &Path, subdir: &str) -> Vec<String> {
    let dir = script_dir.join(subdir);
    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        tracing::warn!("[mod] 插件目录不存在或无法读取: {}", dir.display());
        return Vec::new();
    };
    let mut stems: Vec<String> = read_dir
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("lua") {
                return None;
            }
            let stem = path.file_stem()?.to_str()?.to_string();
            if stem.starts_with('_') {
                return None;
            }
            Some(stem)
        })
        .collect();
    stems.sort();
    stems.into_iter().map(|stem| format!("{subdir}.{stem}")).collect()
}
