use collector_core::{
    center::SharedPointCenter,
    core::point::{PointId, Val},
};
use mlua::{Lua, Result as LuaResult, Table, Value};

/// 将 Val 转为 Lua 值
fn val_to_lua(lua: &Lua, val: &Val) -> LuaResult<Value> {
    match val {
        Val::U8(v) => Ok(Value::Number(*v as f64)),
        Val::I8(v) => Ok(Value::Number(*v as f64)),
        Val::I16(v) => Ok(Value::Number(*v as f64)),
        Val::I32(v) => Ok(Value::Number(*v as f64)),
        Val::U16(v) => Ok(Value::Number(*v as f64)),
        Val::U32(v) => Ok(Value::Number(*v as f64)),
        Val::F64(v) => Ok(Value::Number(*v)),
        Val::List(items) => {
            let t = lua.create_table()?;
            for (i, item) in items.iter().enumerate() {
                t.set(i + 1, val_to_lua(lua, item)?)?;
            }
            Ok(Value::Table(t))
        }
    }
}

/// 将 Lua 值转为 Val（用于 dispatch）
fn lua_to_val(value: Value) -> LuaResult<Val> {
    match value {
        Value::Integer(n) => Ok(Val::I32(n as i32)),
        Value::Number(n) => Ok(Val::F64(n)),
        Value::Boolean(_) => Err(mlua::Error::runtime(
            "dc.dispatch 不支持 boolean 类型，请传入数值",
        )),
        other => Err(mlua::Error::runtime(format!(
            "dc.dispatch 不支持 {} 类型，请传入数值",
            other.type_name()
        ))),
    }
}

pub fn create_table(lua: &Lua, center: SharedPointCenter) -> LuaResult<Table> {
    let table = lua.create_table()?;

    // dc.read_all(dev_id) -> [{id, key, name, value}, ...]
    {
        let c = center.clone();
        table.set(
            "read_all",
            lua.create_function(move |lua, dev_id: String| {
                let points = c.read_all(&dev_id);
                let result = lua.create_table()?;
                for (i, point) in points.iter().enumerate() {
                    let t = lua.create_table()?;
                    t.set("id", point.id)?;
                    t.set("key", point.key)?;
                    t.set("name", point.name)?;
                    t.set("value", val_to_lua(lua, &point.value)?)?;
                    result.set(i + 1, t)?;
                }
                Ok(result)
            })?,
        )?;
    }

    // dc.read(dev_id, point_id) -> {id, key, name, value} | nil
    {
        let c = center.clone();
        table.set(
            "read",
            lua.create_function(move |lua, (dev_id, point_id): (String, PointId)| {
                match c.read(&dev_id, point_id) {
                    None => Ok(Value::Nil),
                    Some(point) => {
                        let t = lua.create_table()?;
                        t.set("id", point.id)?;
                        t.set("key", point.key)?;
                        t.set("name", point.name)?;
                        t.set("value", val_to_lua(lua, &point.value)?)?;
                        Ok(Value::Table(t))
                    }
                }
            })?,
        )?;
    }

    // dc.dev_ids() -> [string, ...]
    {
        let c = center.clone();
        table.set(
            "dev_ids",
            lua.create_function(move |lua, ()| {
                let ids = c.dev_ids();
                let t = lua.create_table()?;
                for (i, id) in ids.iter().enumerate() {
                    t.set(i + 1, id.as_str())?;
                }
                Ok(t)
            })?,
        )?;
    }

    // dc.dispatch(dev_id, point_id, value) (async)
    {
        let c = center.clone();
        table.set(
            "dispatch",
            lua.create_async_function(
                move |_, (dev_id, point_id, value): (String, PointId, Value)| {
                    let c = c.clone();
                    async move {
                        let val = lua_to_val(value)?;
                        let down = collector_core::core::point::DownDataPoint::by_id(point_id, val);
                        c.dispatch(&dev_id, vec![down])
                            .await
                            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                        Ok(())
                    }
                },
            )?,
        )?;
    }

    Ok(table)
}
