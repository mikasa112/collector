use mlua::{Lua, Result as LuaResult, Table};

pub fn create_table(lua: &Lua) -> LuaResult<Table> {
    let table = lua.create_table()?;

    table.set(
        "info",
        lua.create_function(|_, msg: String| {
            tracing::info!("[lua] {}", msg);
            Ok(())
        })?,
    )?;

    table.set(
        "warn",
        lua.create_function(|_, msg: String| {
            tracing::warn!("[lua] {}", msg);
            Ok(())
        })?,
    )?;

    table.set(
        "error",
        lua.create_function(|_, msg: String| {
            tracing::error!("[lua] {}", msg);
            Ok(())
        })?,
    )?;

    Ok(table)
}
