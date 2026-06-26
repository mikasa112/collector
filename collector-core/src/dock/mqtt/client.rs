use std::time::Duration;

use bytes::Bytes;
use rumqttc::{AsyncClient, ClientError, Event, EventLoop, MqttOptions, Packet, QoS};
use tokio::{
    select,
    sync::{Mutex, watch},
    task::JoinHandle,
    time,
};
use tracing::{error, info};

use crate::{
    center::SharedPointCenter,
    config::{MqttRoute, Project},
    core::point::{DownDataPoint, Val},
    dock::mqtt::MqttOverrideStore,
};

#[derive(Debug, thiserror::Error)]
pub enum MqttClientError {
    #[error("Client error: {0}")]
    ClientError(#[from] ClientError),
}

#[derive(Debug, thiserror::Error)]
enum MqttReceiveError {
    #[error("invalid mqtt payload: {0}")]
    Payload(#[from] serde_json::Error),
    #[error("invalid mqtt payload type")]
    PayloadType,
    #[error("invalid mqtt topic: {0}")]
    Topic(String),
}

pub struct MqttClient {
    client: AsyncClient,
    watch_tx: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    pub override_store: MqttOverrideStore,
}

struct MqttClientConf {
    mqtt_host: String,
    mqtt_port: u16,
    mqtt_username: String,
    mqtt_password: String,
    mqtt_yt: String,
    mqtt_yk: String,
    mqtt_routes: Vec<MqttRoute>,
}

impl MqttClient {
    pub fn from_project(
        project: &mut Project,
        center: SharedPointCenter,
    ) -> Result<Option<Self>, MqttClientError> {
        let Some(conf) = MqttClientConf::from_project(project) else {
            return Ok(None);
        };
        Self::new(conf, center).map(Some)
    }

    fn new(conf: MqttClientConf, center: SharedPointCenter) -> Result<Self, MqttClientError> {
        let MqttClientConf {
            mqtt_host,
            mqtt_port,
            mqtt_username,
            mqtt_password,
            mqtt_yt,
            mqtt_yk,
            mqtt_routes,
        } = conf;
        let mut mqttoptions = MqttOptions::new("collector", mqtt_host.as_str(), mqtt_port);
        mqttoptions.set_credentials(mqtt_username.as_str(), mqtt_password.as_str());
        mqttoptions.set_keep_alive(Duration::from_secs(5));
        let (watch_tx, watch_rx) = watch::channel(false);
        let (client, eventloop) = AsyncClient::new(mqttoptions, 10);
        let override_store = MqttOverrideStore::new();
        let event_task = tokio::spawn(receiver(
            mqtt_yt,
            mqtt_yk,
            client.clone(),
            watch_rx,
            center.clone(),
            eventloop,
        ));
        let publish_task = tokio::spawn(publisher(
            watch_tx.subscribe(),
            center.clone(),
            client.clone(),
            mqtt_routes.clone(),
            override_store.clone(),
        ));
        Ok(Self {
            client,
            watch_tx,
            tasks: Mutex::new(vec![event_task, publish_task]),
            override_store,
        })
    }

    pub async fn stop(&self) -> Result<(), MqttClientError> {
        let _ = self.watch_tx.send(true);
        self.client.disconnect().await?;
        let mut task_guard = self.tasks.lock().await;
        for mut handle in task_guard.drain(..) {
            tokio::select! {
                _ = time::sleep(Duration::from_secs(3)) => {
                    handle.abort();
                }
                _ = &mut handle => {}
            }
        }
        info!("MQTT Client Disconnected");
        Ok(())
    }
}

impl MqttClientConf {
    fn from_project(project: &mut Project) -> Option<Self> {
        Some(Self {
            mqtt_host: project.mqtt_host.clone()?,
            mqtt_port: project.mqtt_port?,
            mqtt_username: project.mqtt_username.clone()?,
            mqtt_password: project.mqtt_password.clone()?,
            mqtt_yt: project.mqtt_yt.clone()?,
            mqtt_yk: project.mqtt_yk.clone()?,
            mqtt_routes: project.mqtt_routes.take().unwrap_or_default(),
        })
    }
}

async fn publisher(
    mut publish_stop_rx: watch::Receiver<bool>,
    center: SharedPointCenter,
    client: AsyncClient,
    publish_routes: Vec<MqttRoute>,
    override_store: MqttOverrideStore,
) {
    let mut ticker = time::interval(Duration::from_secs(1));
    loop {
        select! {
            changed = publish_stop_rx.changed() => {
                match changed {
                    Ok(()) if *publish_stop_rx.borrow() => break,
                    Ok(()) => {}
                    Err(_) => break,
                }
            }
            _ = ticker.tick() => {
                publish_routes_task(center.as_ref(), &client, &publish_routes, &override_store)
                    .await;
            }
        }
    }
}

async fn receiver(
    mqtt_yt: String,
    mqtt_yk: String,
    event_client: AsyncClient,
    mut watch_rx: watch::Receiver<bool>,
    event_center: SharedPointCenter,
    mut eventloop: EventLoop,
) {
    if !mqtt_yt.is_empty()
        && let Err(e) = event_client
            .subscribe(mqtt_yt.as_str(), QoS::AtLeastOnce)
            .await
    {
        error!("mqtt subscribe yt error: {:?}", e);
    }
    if !mqtt_yk.is_empty()
        && mqtt_yk != mqtt_yt
        && let Err(e) = event_client
            .subscribe(mqtt_yk.as_str(), QoS::AtLeastOnce)
            .await
    {
        error!("mqtt subscribe yk error: {:?}", e);
    }
    loop {
        select! {
            changed = watch_rx.changed() => {
                match changed {
                    Ok(()) if *watch_rx.borrow() => break,
                    Ok(()) => {}
                    Err(_) => break,
                }
            }
            event = eventloop.poll() => {
                match event {
                    Ok(Event::Incoming(Packet::Publish(p))) => {
                        if let Err(e) = handle_incoming_publish(
                            event_center.as_ref(),
                            p.topic.as_str(),
                            &p.payload,
                        ).await {
                            error!("mqtt receive error: {}", e);
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        error!("mqtt error: {:?}", e);
                        break;
                    }
                }
            }
        }
    }
}

async fn handle_incoming_publish(
    center: &dyn crate::center::PointCenter,
    topic: &str,
    payload: &Bytes,
) -> Result<(), MqttReceiveError> {
    let text = std::str::from_utf8(payload.as_ref()).unwrap_or_default();
    let raw: serde_json::Value = serde_json::from_str(text)?;
    let device_id = parse_device_id_from_topic(topic)?;
    let points = parse_downlink_payload(raw)?;
    if points.is_empty() {
        return Ok(());
    }
    if let Err(e) = center.dispatch(&device_id, points).await {
        error!("mqtt dispatch error on topic {}: {}", topic, e);
    }
    Ok(())
}

fn parse_device_id_from_topic(topic: &str) -> Result<String, MqttReceiveError> {
    let segments: Vec<&str> = topic.split('/').filter(|s| !s.is_empty()).collect();
    let len = segments.len();
    if len < 3 {
        return Err(MqttReceiveError::Topic(topic.to_owned()));
    }
    let mut prefix = segments[len - 3];
    // 这里是鸿合项目使用的
    if prefix == "bank" {
        prefix = "bcu";
    }
    let index: u32 = segments[len - 2]
        .parse()
        .map_err(|_| MqttReceiveError::Topic(topic.to_owned()))?;
    Ok(format!("{}{}", prefix, index + 1))
}

fn parse_downlink_payload(raw: serde_json::Value) -> Result<Vec<DownDataPoint>, MqttReceiveError> {
    let raw_object = match raw {
        serde_json::Value::Object(map) => map,
        _ => return Err(MqttReceiveError::PayloadType),
    };
    let mut points = Vec::with_capacity(raw_object.len());
    for (k, v) in raw_object {
        let value = json_to_val(v).ok_or(MqttReceiveError::PayloadType)?;
        let point = if let Ok(id) = k.parse::<u32>() {
            DownDataPoint::by_id(id, value)
        } else {
            DownDataPoint::by_key(k, value)
        };
        points.push(point);
    }
    Ok(points)
}

fn json_to_val(value: serde_json::Value) -> Option<Val> {
    match value {
        serde_json::Value::Bool(v) => Some(Val::U8(if v { 1 } else { 0 })),
        serde_json::Value::Number(v) => {
            if let Some(n) = v.as_u64() {
                Some(u64_to_val(n))
            } else if let Some(n) = v.as_i64() {
                Some(i64_to_val(n))
            } else {
                v.as_f64().map(Val::F64)
            }
        }
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(json_to_val)
            .collect::<Option<Vec<_>>>()
            .map(Val::List),
        serde_json::Value::Null => None,
        serde_json::Value::String(v) => v.parse::<f64>().ok().map(Val::F64),
        serde_json::Value::Object(_) => None,
    }
}

fn u64_to_val(value: u64) -> Val {
    if let Ok(v) = u8::try_from(value) {
        Val::U8(v)
    } else if let Ok(v) = u16::try_from(value) {
        Val::U16(v)
    } else if let Ok(v) = u32::try_from(value) {
        Val::U32(v)
    } else {
        Val::F64(value as f64)
    }
}

fn i64_to_val(value: i64) -> Val {
    if let Ok(v) = i8::try_from(value) {
        Val::I8(v)
    } else if let Ok(v) = i16::try_from(value) {
        Val::I16(v)
    } else if let Ok(v) = i32::try_from(value) {
        Val::I32(v)
    } else {
        Val::F64(value as f64)
    }
}

async fn publish_routes_task(
    center: &dyn crate::center::PointCenter,
    client: &AsyncClient,
    routes: &[MqttRoute],
    override_store: &MqttOverrideStore,
) {
    for route in routes {
        for rule in &route.rules {
            // 优先使用 Lua 覆盖值
            let payload = if let Some(override_val) = override_store.get(&rule.topic) {
                serde_json::to_vec(&override_val).ok()
            } else {
                let points = center.read_many(route.device_id.as_str(), &rule.point_ids);
                let mut map = serde_json::Map::with_capacity(points.len());
                for point in points {
                    if let Ok(value) = serde_json::to_value(&point.value) {
                        map.insert(point.id.to_string(), value);
                    }
                }
                if map.is_empty() {
                    None
                } else {
                    serde_json::to_vec(&map).ok()
                }
            };
            if let Some(data) = payload {
                let result = client
                    .publish(rule.topic.as_str(), QoS::AtLeastOnce, false, data)
                    .await;
                if let Err(e) = result {
                    error!("MQTT push error:{}", e);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_device_id_from_topic, parse_downlink_payload};

    #[test]
    fn parses_device_id_from_three_segment_topic() {
        assert_eq!(parse_device_id_from_topic("/pcs/0/yt").unwrap(), "pcs1");
    }

    #[test]
    fn parses_device_id_from_four_segment_topic() {
        assert_eq!(parse_device_id_from_topic("/asw/pcs/0/yt").unwrap(), "pcs1");
    }

    #[test]
    fn parses_direct_point_map_payload() {
        let raw = serde_json::json!({
            "1": 12.3,
            "2": 4
        });
        let points = parse_downlink_payload(raw).unwrap();

        assert_eq!(points.len(), 2);
    }
}
