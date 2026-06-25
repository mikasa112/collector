use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use mlua::{Lua, LuaSerdeExt, Table};

pub type LuaStore = Arc<Mutex<HashMap<String, serde_json::Value>>>;

pub fn new_store() -> LuaStore {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn create_store_table(lua: &Lua, store: LuaStore) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    {
        let s = store.clone();
        table.set(
            "set",
            lua.create_function(move |lua, (key, value): (String, mlua::Value)| {
                let json: serde_json::Value = lua.from_value(value)?;
                s.lock().unwrap().insert(key, json);
                Ok(())
            })?,
        )?;
    }

    {
        let s = store.clone();
        table.set(
            "get",
            lua.create_function(move |lua, key: String| {
                let val = s.lock().unwrap().get(&key).cloned();
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
            store.lock().unwrap().remove(&key);
            Ok(())
        })?,
    )?;

    Ok(table)
}
