use std::{
    collections::{HashMap, HashSet},
    path::Path,
};

use mlua::{Lua, Table, Value};

use crate::mod_engine::api::db::DbPerm;

/// 模块注册总纲文件名。以 `_` 开头，天然被 `script_loader::scan_dir` 当作辅助文件跳过，
/// 不会被误当作可运行模块加载——它本身只是一段返回文件名列表的 Lua 代码，
/// 允许写任意前置逻辑（比如按条件判断），最终 `return` 一个字符串数组即可。
pub const MANIFEST_FILE: &str = "_manifest.lua";

/// 总纲中的一项：文件名 + 该脚本声明的数据库表权限
#[derive(Debug, Clone, Default)]
struct Entry {
    db: DbPerm,
}

#[derive(Debug, Clone, Default)]
pub struct Manifest {
    enabled: HashMap<String, Entry>,
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
            Ok(entries) => Some(Self {
                enabled: entries.into_iter().collect(),
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

    /// 在独立的 Lua VM 中执行总纲脚本，取其 `return` 的数组。
    /// 数组元素可以是文件名字符串，或 `{ file = "x.lua", db = { read = {...}, write = {...} } }`
    fn eval(source: &str) -> mlua::Result<Vec<(String, Entry)>> {
        let lua = Lua::new();
        let list: Table = lua.load(source).eval()?;
        let mut out = Vec::new();
        for item in list.sequence_values::<Value>() {
            match item? {
                Value::String(s) => out.push((s.to_str()?.to_string(), Entry::default())),
                Value::Table(t) => {
                    let file: String = t.get("file").map_err(|_| {
                        mlua::Error::runtime("总纲表项缺少 file 字段（字符串）")
                    })?;
                    let db = match t.get::<Value>("db")? {
                        Value::Nil => DbPerm::default(),
                        Value::Table(d) => DbPerm::new(
                            name_list(&d, "read")?,
                            name_list(&d, "write")?,
                        ),
                        _ => return Err(mlua::Error::runtime("db 必须是 table")),
                    };
                    out.push((file, Entry { db }));
                }
                _ => {
                    return Err(mlua::Error::runtime(
                        "总纲元素必须是文件名字符串或 { file=..., db=... } 表",
                    ));
                }
            }
        }
        Ok(out)
    }

    /// 传入脚本文件名（不含目录，如 "main.lua"）判断是否允许执行
    pub fn is_enabled(&self, filename: &str) -> bool {
        self.enabled.contains_key(filename)
    }

    /// 该脚本声明的数据库表权限；未声明返回空权限（拒绝一切表访问）
    pub fn db_perm(&self, filename: &str) -> DbPerm {
        self.enabled
            .get(filename)
            .map(|e| e.db.clone())
            .unwrap_or_default()
    }
}

/// 读取 `{ read = {"t_a","t_b"} }` 中的表名列表，缺省为空
fn name_list(t: &Table, key: &str) -> mlua::Result<HashSet<String>> {
    match t.get::<Value>(key)? {
        Value::Nil => Ok(HashSet::new()),
        Value::Table(l) => l
            .sequence_values::<String>()
            .map(|r| r.map(|s| s.to_ascii_lowercase()))
            .collect(),
        _ => Err(mlua::Error::runtime(format!("db.{key} 必须是表名数组"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> Manifest {
        Manifest {
            enabled: Manifest::eval(src).unwrap().into_iter().collect(),
        }
    }

    #[test]
    fn plain_string_list_still_works_and_has_no_db_perm() {
        let m = parse(r#"return { "a.lua", "b.lua" }"#);
        assert!(m.is_enabled("a.lua") && m.is_enabled("b.lua") && !m.is_enabled("c.lua"));
        let p = m.db_perm("a.lua");
        assert!(!p.can_read("t_alarm") && !p.can_write("t_alarm"));
    }

    #[test]
    fn table_entry_declares_db_perm() {
        let m = parse(
            r#"return {
              "plain.lua",
              { file = "x.lua", db = { read = {"T_Alarm"}, write = {"t_plan_curve_detail"} } },
              { file = "nodb.lua" },
            }"#,
        );
        assert!(m.is_enabled("x.lua") && m.is_enabled("nodb.lua") && m.is_enabled("plain.lua"));
        let p = m.db_perm("x.lua");
        assert!(p.can_read("t_alarm"), "表名应大小写不敏感");
        assert!(!p.can_write("t_alarm"));
        assert!(p.can_write("t_plan_curve_detail") && p.can_read("t_plan_curve_detail"));
        assert!(!m.db_perm("nodb.lua").can_read("t_alarm"));
    }

    #[test]
    fn wildcard_and_unknown_script() {
        let m = parse(r#"return { { file = "x.lua", db = { write = {"*"} } } }"#);
        assert!(m.db_perm("x.lua").can_write("any_table"));
        assert!(!m.db_perm("missing.lua").can_read("any_table"));
    }

    #[test]
    fn malformed_entries_are_errors() {
        for src in [
            r#"return { 123 }"#,
            r#"return { { db = {} } }"#,
            r#"return { { file = "x.lua", db = "t_a" } }"#,
            r#"return { { file = "x.lua", db = { read = "t_a" } } }"#,
            r#"return { { file = "x.lua", db = { write = { 1, {} } } } }"#,
            r#"return "x.lua""#,
        ] {
            assert!(Manifest::eval(src).is_err(), "应报错: {src}");
        }
    }
}
