use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use mlua::{Function, Lua, RegistryKey, Table, UserData, UserDataMethods, Value};
use rumqttc::{AsyncClient, Event, EventLoop, MqttOptions, Packet, QoS};
use tokio::{sync::mpsc, task::JoinHandle};

use crate::mod_engine::{api::mqtt::lua_to_json, engine::EngineCmd};

/// 订阅记录：conn_id + topic filter -> 回调，引擎级共享（一个脚本可开多个连接）
pub struct Subscription {
    pub conn_id: u64,
    pub filter: String,
    pub callback: RegistryKey,
}

/// 脚本已建立的连接，脚本卸载/热更新时用于统一断开，避免连接泄漏
pub struct ConnEntry {
    pub client: AsyncClient,
    pub task: JoinHandle<()>,
}

pub type MqttSubs = Arc<Mutex<Vec<Subscription>>>;
pub type MqttConns = Arc<Mutex<Vec<ConnEntry>>>;

/// 判断 topic 是否匹配 MQTT 订阅过滤器（支持标准的 `+`/`#` 通配符）
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    let filter_parts: Vec<&str> = filter.split('/').collect();
    let topic_parts: Vec<&str> = topic.split('/').collect();
    matches_parts(&filter_parts, &topic_parts)
}

fn matches_parts(filter: &[&str], topic: &[&str]) -> bool {
    match (filter.first(), topic.first()) {
        (Some(&"#"), _) => true,
        (Some(&"+"), Some(_)) => matches_parts(&filter[1..], &topic[1..]),
        (Some(f), Some(t)) if f == t => matches_parts(&filter[1..], &topic[1..]),
        (None, None) => true,
        _ => false,
    }
}

fn qos_from_u8(v: u8) -> QoS {
    match v {
        1 => QoS::AtLeastOnce,
        2 => QoS::ExactlyOnce,
        _ => QoS::AtMostOnce,
    }
}

/// 将 Lua 值转为 MQTT payload 字节：字符串按原始字节发送，table 走 JSON 编码，其余转字符串
fn lua_value_to_payload(value: Value) -> mlua::Result<Vec<u8>> {
    match value {
        Value::String(s) => Ok(s.as_bytes().to_vec()),
        Value::Table(_) => {
            let json = lua_to_json(value)?;
            serde_json::to_vec(&json).map_err(|e| mlua::Error::runtime(e.to_string()))
        }
        Value::Integer(n) => Ok(n.to_string().into_bytes()),
        Value::Number(n) => Ok(n.to_string().into_bytes()),
        Value::Boolean(b) => Ok(b.to_string().into_bytes()),
        other => Err(mlua::Error::runtime(format!(
            "mqtt publish 不支持 {} 类型的 payload",
            other.type_name()
        ))),
    }
}

fn opt_u8(opts: &Option<Table>, key: &str, default: u8) -> u8 {
    opts.as_ref()
        .and_then(|t| t.get::<u8>(key).ok())
        .unwrap_or(default)
}

fn opt_bool(opts: &Option<Table>, key: &str, default: bool) -> bool {
    opts.as_ref()
        .and_then(|t| t.get::<bool>(key).ok())
        .unwrap_or(default)
}

/// 脚本持有的 MQTT 连接句柄，作为 UserData 返回给 Lua
#[derive(Clone)]
pub struct MqttConnHandle {
    id: u64,
    client: AsyncClient,
    subs: MqttSubs,
}

impl UserData for MqttConnHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method(
            "publish",
            |_, this, (topic, payload, opts): (String, Value, Option<Table>)| async move {
                let qos = qos_from_u8(opt_u8(&opts, "qos", 0));
                let retain = opt_bool(&opts, "retain", false);
                let bytes = lua_value_to_payload(payload)?;
                this.client
                    .publish(topic, qos, retain, bytes)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))
            },
        );

        methods.add_async_method(
            "subscribe",
            |lua, this, (filter, callback, opts): (String, Function, Option<Table>)| async move {
                let qos = qos_from_u8(opt_u8(&opts, "qos", 0));
                this.client
                    .subscribe(filter.clone(), qos)
                    .await
                    .map_err(|e| mlua::Error::runtime(e.to_string()))?;
                let key = lua.create_registry_value(callback)?;
                this.subs.lock().unwrap().push(Subscription {
                    conn_id: this.id,
                    filter,
                    callback: key,
                });
                Ok(())
            },
        );

        methods.add_async_method("unsubscribe", |_, this, filter: String| async move {
            this.client
                .unsubscribe(filter.clone())
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))?;
            this.subs
                .lock()
                .unwrap()
                .retain(|s| !(s.conn_id == this.id && s.filter == filter));
            Ok(())
        });

        methods.add_async_method("disconnect", |_, this, ()| async move {
            this.subs.lock().unwrap().retain(|s| s.conn_id != this.id);
            this.client
                .disconnect()
                .await
                .map_err(|e| mlua::Error::runtime(e.to_string()))
        });
    }
}

/// 后台持续轮询连接事件循环，把收到的 Publish 转发回引擎主循环处理。
/// 网络抖动/出错时只记录日志继续 poll（rumqttc 会自动重连），不能因为一次网络错误就中断脚本的订阅。
async fn poll_loop(conn_id: u64, mut eventloop: EventLoop, tx: mpsc::UnboundedSender<EngineCmd>) {
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Packet::Publish(p))) => {
                let _ = tx.send(EngineCmd::MqttMessage {
                    conn_id,
                    topic: p.topic.clone(),
                    payload: p.payload.to_vec(),
                });
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("[mod] mqtt 连接 {} 错误: {}", conn_id, e);
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

/// 创建 `mqtt` 全局表：脚本通过 `mqtt.connect` 自行发起独立的 MQTT 连接，
/// 与 dock 层系统级发布逻辑（`override` 表）完全独立、互不感知。
pub fn create_mqtt_conn_table(
    lua: &Lua,
    tx: mpsc::UnboundedSender<EngineCmd>,
    mqtt_subs: MqttSubs,
    mqtt_conns: MqttConns,
    next_id: Arc<AtomicU64>,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;

    table.set(
        "connect",
        lua.create_async_function(move |lua, opts: Table| {
            let tx = tx.clone();
            let mqtt_subs = mqtt_subs.clone();
            let mqtt_conns = mqtt_conns.clone();
            let next_id = next_id.clone();
            async move {
                let host: String = opts.get("host")?;
                let port: u16 = opts.get("port").unwrap_or(1883);
                let client_id: String = opts
                    .get("client_id")
                    .unwrap_or_else(|_| format!("script-{}", next_id.load(Ordering::Relaxed)));
                let keepalive: u64 = opts.get("keepalive").unwrap_or(30);
                // rumqttc 默认收发包上限仅 10KB，对上送全量数据点位的场景明显偏小，
                // 这里把默认值放大到 256KB；脚本仍可通过 max_packet_size 覆盖。
                let max_packet_size: usize = opts.get("max_packet_size").unwrap_or(256 * 1024);

                let mut mqtt_options = MqttOptions::new(client_id, host, port);
                mqtt_options.set_keep_alive(Duration::from_secs(keepalive));
                mqtt_options.set_max_packet_size(max_packet_size, max_packet_size);
                if let (Ok(username), Ok(password)) = (
                    opts.get::<String>("username"),
                    opts.get::<String>("password"),
                ) {
                    mqtt_options.set_credentials(username, password);
                }

                let (client, mut eventloop) = AsyncClient::new(mqtt_options, 10);
                let conn_id = next_id.fetch_add(1, Ordering::Relaxed);

                // 等待首次 ConnAck 才认为连接建立成功，带超时保护
                let wait_connected = async {
                    loop {
                        match eventloop.poll().await {
                            Ok(Event::Incoming(Packet::ConnAck(_))) => return Ok(()),
                            Ok(_) => continue,
                            Err(e) => return Err(e.to_string()),
                        }
                    }
                };
                match tokio::time::timeout(Duration::from_secs(5), wait_connected).await {
                    Err(_) => return Ok((Value::Nil, Some("mqtt 连接超时".to_string()))),
                    Ok(Err(e)) => return Ok((Value::Nil, Some(e))),
                    Ok(Ok(())) => {}
                }

                let task = tokio::spawn(poll_loop(conn_id, eventloop, tx));
                mqtt_conns.lock().unwrap().push(ConnEntry {
                    client: client.clone(),
                    task,
                });

                let handle = MqttConnHandle {
                    id: conn_id,
                    client,
                    subs: mqtt_subs,
                };
                let ud = lua.create_userdata(handle)?;
                Ok((Value::UserData(ud), None))
            }
        })?,
    )?;

    Ok(table)
}

#[cfg(test)]
mod tests {
    use super::topic_matches;

    #[test]
    fn matches_exact_topic() {
        assert!(topic_matches("sport/tennis", "sport/tennis"));
        assert!(!topic_matches("sport/tennis", "sport/football"));
    }

    #[test]
    fn matches_multi_level_wildcard() {
        assert!(topic_matches("sport/#", "sport"));
        assert!(topic_matches("sport/#", "sport/tennis"));
        assert!(topic_matches("sport/#", "sport/tennis/player1"));
        assert!(!topic_matches("sport/#", "other"));
    }

    #[test]
    fn matches_single_level_wildcard() {
        assert!(topic_matches("sport/+/score", "sport/tennis/score"));
        assert!(!topic_matches("sport/+/score", "sport/tennis/x/score"));
        assert!(!topic_matches("sport/+/score", "sport/score"));
    }
}
