use mlua::{Lua, Table};

#[allow(dead_code)]
pub fn create_mqtt_table(lua: &Lua) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    Ok(table)
}
