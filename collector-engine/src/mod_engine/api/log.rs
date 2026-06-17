use std::fmt::Display;

use mlua::{Lua, Table, Value};

struct DisplayValue(mlua::Value);

impl Display for DisplayValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.0 {
            Value::Nil => write!(f, "nil"),
            Value::Boolean(b) => write!(f, "{}", b),
            Value::Integer(n) => write!(f, "{}", n),
            Value::Number(n) => write!(f, "{}", n),
            Value::String(s) => write!(f, "{}", s.to_string_lossy()),
            Value::Table(_) => write!(f, "(table)"),
            Value::Function(_) => write!(f, "(function)"),
            other => write!(f, "({})", other.type_name()),
        }
    }
}

pub fn create_log_table(lua: &Lua) -> mlua::Result<Table> {
    let log_table = lua.create_table()?;
    log_table.set(
        "debug",
        lua.create_function(|_, msg: Value| {
            tracing::debug!("[mod] {}", DisplayValue(msg));
            Ok(())
        })?,
    )?;
    log_table.set(
        "info",
        lua.create_function(|_, msg: Value| {
            tracing::info!("[mod] {}", DisplayValue(msg));
            Ok(())
        })?,
    )?;
    log_table.set(
        "warn",
        lua.create_function(|_, msg: Value| {
            tracing::warn!("[mod] {}", DisplayValue(msg));
            Ok(())
        })?,
    )?;
    log_table.set(
        "error",
        lua.create_function(|_, msg: Value| {
            tracing::error!("[mod] {}", DisplayValue(msg));
            Ok(())
        })?,
    )?;
    Ok(log_table)
}
