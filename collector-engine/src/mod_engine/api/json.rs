use mlua::{Lua, Table, Value};

use crate::mod_engine::api::mqtt::lua_to_json;

/// 将 serde_json::Value 转为 mlua::Value
fn json_to_lua(lua: &Lua, value: serde_json::Value) -> mlua::Result<Value> {
    match value {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else {
                Ok(Value::Number(n.as_f64().unwrap_or_default()))
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(&s)?)),
        serde_json::Value::Array(arr) => {
            let table = lua.create_table()?;
            for (i, item) in arr.into_iter().enumerate() {
                table.set(i + 1, json_to_lua(lua, item)?)?;
            }
            Ok(Value::Table(table))
        }
        serde_json::Value::Object(map) => {
            let table = lua.create_table()?;
            for (k, v) in map {
                table.set(k, json_to_lua(lua, v)?)?;
            }
            Ok(Value::Table(table))
        }
    }
}

pub fn create_json_table(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "decode",
        lua.create_function(|lua, s: String| {
            let value: serde_json::Value =
                serde_json::from_str(&s).map_err(|e| mlua::Error::runtime(e.to_string()))?;
            json_to_lua(lua, value)
        })?,
    )?;

    table.set(
        "encode",
        lua.create_function(|_, value: Value| {
            let json = lua_to_json(value)?;
            serde_json::to_string(&json).map_err(|e| mlua::Error::runtime(e.to_string()))
        })?,
    )?;

    Ok(table)
}
