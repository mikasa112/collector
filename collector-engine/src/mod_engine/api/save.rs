use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use mlua::{Lua, LuaSerdeExt, Table};

type SaveData = Arc<Mutex<HashMap<String, serde_json::Value>>>;

fn load_from_disk(data_file: &PathBuf) -> HashMap<String, serde_json::Value> {
    match std::fs::read_to_string(data_file) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(map) => map,
            Err(e) => {
                tracing::warn!(
                    "[mod] 存档文件解析失败 {}: {}，从空存档开始",
                    data_file.display(),
                    e
                );
                HashMap::new()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => HashMap::new(),
        Err(e) => {
            tracing::warn!(
                "[mod] 存档文件读取失败 {}: {}，从空存档开始",
                data_file.display(),
                e
            );
            HashMap::new()
        }
    }
}

fn flush_to_disk(data_file: &PathBuf, data: &HashMap<String, serde_json::Value>) {
    match serde_json::to_string(data) {
        Ok(json) => {
            if let Err(e) = std::fs::write(data_file, json) {
                tracing::warn!("[mod] 存档写入失败 {}: {}", data_file.display(), e);
            }
        }
        Err(e) => {
            tracing::warn!("[mod] 存档序列化失败 {}: {}", data_file.display(), e);
        }
    }
}

/// 按脚本文件名隔离、落盘的持久化存档表，跨脚本热重载/进程重启保留数据。
/// 区别于纯内存共享的 `store`：`save` 每个脚本各自独立，且每次写入都同步落盘。
pub fn create_save_table(lua: &Lua, data_file: PathBuf) -> mlua::Result<Table> {
    let data: SaveData = Arc::new(Mutex::new(load_from_disk(&data_file)));
    let table = lua.create_table()?;

    {
        let data = data.clone();
        let data_file = data_file.clone();
        table.set(
            "set",
            lua.create_function(move |lua, (key, value): (String, mlua::Value)| {
                let json: serde_json::Value = lua.from_value(value)?;
                let mut map = data.lock().unwrap();
                map.insert(key, json);
                flush_to_disk(&data_file, &map);
                Ok(())
            })?,
        )?;
    }

    {
        let data = data.clone();
        table.set(
            "get",
            lua.create_function(move |lua, key: String| {
                let val = data.lock().unwrap().get(&key).cloned();
                match val {
                    None => Ok(mlua::Value::Nil),
                    Some(json) => lua.to_value(&json),
                }
            })?,
        )?;
    }

    table.set(
        "del",
        lua.create_function(move |_, key: String| {
            let mut map = data.lock().unwrap();
            map.remove(&key);
            flush_to_disk(&data_file, &map);
            Ok(())
        })?,
    )?;

    Ok(table)
}
