use std::{collections::HashSet, path::Path};

use mlua::Lua;

/// 模块注册总纲文件名。以 `_` 开头，天然被 `script_loader::scan_dir` 当作辅助文件跳过，
/// 不会被误当作可运行模块加载——它本身只是一段返回文件名列表的 Lua 代码，
/// 允许写任意前置逻辑（比如按条件判断），最终 `return` 一个字符串数组即可。
pub const MANIFEST_FILE: &str = "_manifest.lua";

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    enabled: HashSet<String>,
}

impl Manifest {
    /// 加载脚本目录下的注册总纲。
    ///
    /// 文件不存在时返回 `None`，表示未启用注册限制（兼容旧行为：目录下的脚本按原逻辑全部允许执行）。
    /// 文件存在但执行/返回值格式出错时返回一张空总纲（`Some` 但内容为空），
    /// 即所有脚本都不允许执行——总纲一旦启用就必须能正确给出名单，避免因为文件写错而静默放行。
    pub async fn load(script_dir: &Path) -> Option<Self> {
        let path = script_dir.join(MANIFEST_FILE);
        let source = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
            Err(e) => {
                tracing::error!(
                    "[mod] 读取模块注册总纲失败 {}: {}，视为空总纲（所有脚本均不执行）",
                    path.display(),
                    e
                );
                return Some(Self::default());
            }
        };
        match Self::eval(&source) {
            Ok(names) => Some(Self {
                enabled: names.into_iter().collect(),
            }),
            Err(e) => {
                tracing::error!(
                    "[mod] 模块注册总纲执行/返回值格式错误 {}: {}，视为空总纲（所有脚本均不执行）",
                    path.display(),
                    e
                );
                Some(Self::default())
            }
        }
    }

    /// 在独立的 Lua VM 中执行总纲脚本，取其 `return` 的字符串数组
    fn eval(source: &str) -> mlua::Result<Vec<String>> {
        let lua = Lua::new();
        lua.load(source).eval::<Vec<String>>()
    }

    /// 传入脚本文件名（不含目录，如 "main.lua"）判断是否允许执行
    pub fn is_enabled(&self, filename: &str) -> bool {
        self.enabled.contains(filename)
    }
}
