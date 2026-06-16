use mlua::{Lua, Table};

pub fn create_log_table(lua: &Lua) -> mlua::Result<Table> {
    let log_table = lua.create_table()?;
    log_table.set(
        "info",
        lua.create_function(|_, msg: String| {
            tracing::info!("[mod] {}", msg);
            Ok(())
        })?,
    )?;
    log_table.set(
        "warn",
        lua.create_function(|_, msg: String| {
            tracing::warn!("[mod] {}", msg);
            Ok(())
        })?,
    )?;
    log_table.set(
        "error",
        lua.create_function(|_, msg: String| {
            tracing::error!("[mod] {}", msg);
            Ok(())
        })?,
    )?;
    Ok(log_table)
}
