use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use collector_core::{
    center::SharedPointCenter,
    core::point::{DataPoint, PointId, Val},
};
use mlua::{Lua, Table, Value};
use tokio::sync::mpsc;

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

fn val_as_u16(val: &Val) -> Option<u16> {
    match val {
        Val::U16(v) => Some(*v),
        Val::U8(v) => Some(*v as u16),
        Val::I16(v) => Some(*v as u16),
        Val::U32(v) => Some(*v as u16),
        Val::I32(v) => Some(*v as u16),
        _ => None,
    }
}

fn append_status_and_faults(lua: &Lua, t: &Table, point: &DataPoint) -> mlua::Result<()> {
    if let Some(status_words) = point.status_word
        && let Some(raw) = val_as_u16(&point.value)
        && let Some(sw) = status_words.words.get(&raw)
    {
        let st = lua.create_table()?;
        st.set("zh", sw.zh)?;
        st.set("en", sw.en)?;
        t.set("status", st)?;
    }
    if let Some(warn_bits) = point.warn_bits
        && let Some(raw) = val_as_u16(&point.value)
    {
        let faults = lua.create_table()?;
        let mut idx = 1usize;
        for bit in 0..16u16 {
            if raw & (1 << bit) != 0 {
                let wb = &warn_bits.bits[bit as usize];
                let ft = lua.create_table()?;
                ft.set("bit", bit)?;
                ft.set("zh", wb.zh)?;
                ft.set("en", wb.en)?;
                ft.set("level", wb.level as u8)?;
                faults.set(idx, ft)?;
                idx += 1;
            }
        }
        t.set("faults", faults)?;
    }
    Ok(())
}

pub fn create_dc_table(
    lua: &Lua,
    center: SharedPointCenter,
    watch_tx: mpsc::UnboundedSender<(String, Arc<[DataPoint]>)>,
) -> mlua::Result<Table> {
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
                    append_status_and_faults(lua, &t, point)?;
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
                        append_status_and_faults(lua, &t, &point)?;
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
    {
        let c = center.clone();
        dc_table.set(
            "dispatch",
            lua.create_async_function(
                move |_, (dev_id, point_mark, value): (String, Value, Value)| {
                    let c = c.clone();
                    async move {
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
                        c.dispatch(&dev_id, vec![down])
                            .await
                            .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                        Ok(())
                    }
                },
            )?,
        )?;
    }

    // dc.watch(dev_id) — 订阅设备数据变化，变化时触发 "dc:changed" 事件
    {
        let c = center.clone();
        let watched: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        dc_table.set(
            "watch",
            lua.create_function(move |_, dev_id: String| {
                let mut set = watched.lock().unwrap();
                if set.contains(&dev_id) {
                    return Ok(());
                }
                match c.subscribe(&dev_id) {
                    None => {
                        tracing::warn!("[mod] dc.watch: 设备 {} 尚未注册，忽略", dev_id);
                    }
                    Some(mut rx) => {
                        set.insert(dev_id.clone());
                        let tx = watch_tx.clone();
                        let dev_id_task = dev_id.clone();
                        tokio::spawn(async move {
                            while rx.changed().await.is_ok() {
                                let snap = rx.borrow_and_update().clone();
                                if tx.send((dev_id_task.clone(), snap)).is_err() {
                                    break;
                                }
                            }
                        });
                        tracing::info!("[mod] dc.watch: 开始监听设备 {}", dev_id);
                    }
                }
                Ok(())
            })?,
        )?;
    }

    Ok(dc_table)
}
