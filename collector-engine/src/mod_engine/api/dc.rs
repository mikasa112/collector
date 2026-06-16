use collector_core::{
    center::SharedPointCenter,
    core::point::{PointId, Val},
};
use mlua::{Lua, Table, Value};

/// 将 Val 转为 Lua 值
fn val_to_lua(lua: &Lua, val: &Val) -> mlua::Result<Value> {
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
fn lua_to_val(value: Value) -> mlua::Result<Val> {
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

pub fn create_dc_table(lua: &Lua, center: SharedPointCenter) -> mlua::Result<Table> {
    let dc_table = lua.create_table()?;

    // dc.read_all(dev_id) -> [{id, key, name, value}, ...]
    {
        let c = center.clone();
        dc_table.set(
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
        dc_table.set(
            "read",
            lua.create_function(move |lua, (dev_id, point_mark): (String, Value)| {
                let point = match point_mark {
                    Value::Integer(point_id) => c.read(&dev_id, point_id as PointId),
                    Value::String(point_key) => {
                        c.read_by_key(&dev_id, &point_key.to_string_lossy())
                    }
                    _ => return Err(mlua::Error::runtime("point_mark 必须是整数或字符串")),
                };

                match point {
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
        dc_table.set(
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

    // dc.dispatch(dev_id, point_mark, value)
    // 底层是 mpsc channel send（微秒级），用 block_on 在阻塞线程里同步等待，
    // 避免在协程调度器（同步 resume）中使用 async function 导致永久挂起。
    // Handle::current() 在闭包内懒求值，避免 create 时不在 tokio 上下文中 panic。
    {
        let c = center.clone();
        dc_table.set(
            "dispatch",
            lua.create_function(
                move |_, (dev_id, point_mark, value): (String, Value, Value)| {
                    let val = lua_to_val(value)?;
                    let down = match point_mark {
                        Value::Integer(point_id) => {
                            collector_core::core::point::DownDataPoint::by_id(
                                point_id as PointId,
                                val,
                            )
                        }
                        Value::String(point_key) => {
                            let k = point_key.to_string_lossy();
                            collector_core::core::point::DownDataPoint::by_key(k, val)
                        }
                        _ => return Err(mlua::Error::runtime("point_mark 必须是整数或字符串")),
                    };
                    let rt = tokio::runtime::Handle::try_current().map_err(|_| {
                        mlua::Error::runtime("dc.dispatch 必须在 tokio 运行时中调用")
                    })?;
                    rt.block_on(c.dispatch(&dev_id, vec![down]))
                        .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                    Ok(())
                },
            )?,
        )?;
    }

    Ok(dc_table)
}
