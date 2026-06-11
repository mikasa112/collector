use mlua::{Lua, Result as LuaResult};

pub mod datacenter;
pub mod logging;

/// 将所有 API 注册到 Lua 全局环境
pub fn register_all(lua: &Lua, center: collector_core::center::SharedPointCenter) -> LuaResult<()> {
    let globals = lua.globals();

    // dc.*
    let dc_table = datacenter::create_table(lua, center)?;
    globals.set("dc", dc_table)?;

    // log.*
    let log_table = logging::create_table(lua)?;
    globals.set("log", log_table)?;

    // sleep(ms)
    let sleep_fn = lua.create_async_function(|_, ms: u64| async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
        Ok(())
    })?;
    globals.set("sleep", sleep_fn)?;

    Ok(())
}

/// 从 Lua 全局环境中读取并调用指定函数（如果存在）
pub async fn call_hook(lua: &Lua, name: &str) -> mlua::Result<()> {
    let globals = lua.globals();
    let func: Option<mlua::Function> = globals.get(name)?;
    if let Some(f) = func {
        f.call_async::<()>(()).await?;
    }
    Ok(())
}
