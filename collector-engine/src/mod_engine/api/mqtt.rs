use std::sync::{Arc, Mutex};

use collector_core::dock::mqtt::MqttOverrideStore;
use mlua::{Lua, Table, Value};

/// 将 mlua::Value 转为 serde_json::Value
fn lua_to_json(value: Value) -> mlua::Result<serde_json::Value> {
    match value {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(b) => Ok(serde_json::Value::Bool(b)),
        Value::Integer(n) => Ok(serde_json::Value::Number(n.into())),
        Value::Number(f) => {
            let n = serde_json::Number::from_f64(f)
                .ok_or_else(|| mlua::Error::runtime("浮点数无法序列化为 JSON（NaN/Inf）"))?;
            Ok(serde_json::Value::Number(n))
        }
        Value::String(s) => Ok(serde_json::Value::String(s.to_string_lossy().to_owned())),
        Value::Table(t) => {
            // 单次遍历收集所有键值对，同时判断是否为连续整数键数组
            let mut entries: Vec<(Value, Value)> = Vec::new();
            let mut max_seq = 0usize; // 追踪连续整数键最大值
            let mut is_array = true;

            for pair in t.pairs::<Value, Value>() {
                let (k, v) = pair?;
                if is_array {
                    match &k {
                        Value::Integer(i) if *i >= 1 => {
                            max_seq = max_seq.max(*i as usize);
                        }
                        _ => is_array = false,
                    }
                }
                entries.push((k, v));
            }

            // 空 table 或键不连续（max_seq != entries.len()）都走对象分支
            let is_array = is_array && !entries.is_empty() && max_seq == entries.len();

            if is_array {
                let mut arr = vec![serde_json::Value::Null; max_seq];
                for (k, v) in entries {
                    let i = match k {
                        Value::Integer(i) => i as usize - 1,
                        _ => unreachable!(),
                    };
                    arr[i] = lua_to_json(v)?;
                }
                Ok(serde_json::Value::Array(arr))
            } else {
                let mut map = serde_json::Map::with_capacity(entries.len());
                for (k, v) in entries {
                    let key = match k {
                        Value::String(s) => s.to_string_lossy().to_owned(),
                        Value::Integer(n) => n.to_string(),
                        Value::Number(f) => f.to_string(),
                        other => {
                            return Err(mlua::Error::runtime(format!(
                                "table key 类型不支持 JSON 序列化: {}",
                                other.type_name()
                            )));
                        }
                    };
                    map.insert(key, lua_to_json(v)?);
                }
                Ok(serde_json::Value::Object(map))
            }
        }
        other => Err(mlua::Error::runtime(format!(
            "mqtt.publish 不支持 {} 类型的 payload",
            other.type_name()
        ))),
    }
}

pub fn create_mqtt_table(
    lua: &Lua,
    store: MqttOverrideStore,
    owned_topics: Arc<Mutex<Vec<String>>>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    {
        let store = store.clone();
        let owned_topics = owned_topics.clone();
        table.set(
            "set",
            lua.create_function(move |_, (topic, value): (String, Value)| {
                let json = lua_to_json(value)?;
                store.set(topic.clone(), json);
                let mut topics = owned_topics.lock().unwrap();
                if !topics.contains(&topic) {
                    topics.push(topic);
                }
                Ok(())
            })?,
        )?;
    }

    table.set(
        "clear",
        lua.create_function(move |_, topic: String| {
            owned_topics.lock().unwrap().retain(|t| t != &topic);
            store.clear(&topic);
            Ok(())
        })?,
    )?;

    Ok(table)
}
