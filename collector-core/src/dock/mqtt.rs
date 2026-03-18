use std::time::Duration;

use bytes::Bytes;
use rumqttc::{AsyncClient, ClientError, Event, MqttOptions, Packet, QoS};
use std::collections::BTreeMap;
use tokio::{
    select,
    sync::{Mutex, watch},
    task::JoinHandle,
    time,
};
use tracing::{error, info};

use crate::{
    center::{Center, global_center},
    config::{MqttRoute, Project},
    core::point::{DataPoint, Val},
    dev::Identifiable,
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
    #[error("invalid point value for id {point_id}")]
    PointValue { point_id: u32 },
}

pub struct MqttClient {
    client: AsyncClient,
    watch_tx: watch::Sender<bool>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
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

struct DevIdRef<'a>(&'a str);

impl Identifiable for DevIdRef<'_> {
    fn id(&self) -> &str {
        self.0
    }
}

impl MqttClient {
    pub fn from_project(project: &mut Project) -> Result<Option<Self>, MqttClientError> {
        let Some(conf) = MqttClientConf::from_project(project) else {
            return Ok(None);
        };
        Self::new(conf).map(Some)
    }

    fn new(conf: MqttClientConf) -> Result<Self, MqttClientError> {
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
        let (watch_tx, mut watch_rx) = watch::channel(false);
        let (client, mut eventloop) = AsyncClient::new(mqttoptions, 10);
        let event_client = client.clone();
        let event_task = tokio::spawn(async move {
            if !mqtt_yt.is_empty()
                && let Err(e) = event_client
                    .subscribe(mqtt_yt.clone(), QoS::AtLeastOnce)
                    .await
            {
                error!("mqtt subscribe yt error: {:?}", e);
            }
            if !mqtt_yk.is_empty()
                && mqtt_yk != mqtt_yt
                && let Err(e) = event_client
                    .subscribe(mqtt_yk.clone(), QoS::AtLeastOnce)
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
                                if let Err(e) = handle_incoming_publish(p.topic.as_str(), &p.payload).await {
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
        });
        let publish_client = client.clone();
        let publish_routes = mqtt_routes;
        let mut publish_stop_rx = watch_tx.subscribe();
        let publish_task = tokio::spawn(async move {
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
                        publish_routes_task(&publish_client, &publish_routes).await;
                    }
                }
            }
        });
        Ok(Self {
            client,
            watch_tx,
            tasks: Mutex::new(vec![event_task, publish_task]),
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

async fn handle_incoming_publish(topic: &str, payload: &Bytes) -> Result<(), MqttReceiveError> {
    let text = std::str::from_utf8(payload.as_ref()).unwrap_or_default();
    let raw: serde_json::Value = serde_json::from_str(text)?;
    let device_id = parse_device_id_from_topic(topic)?;
    let points = parse_downlink_payload(raw)?;
    if points.is_empty() {
        return Ok(());
    }
    let dev = DevIdRef(device_id);
    if let Err(e) = global_center().dispatch(&dev, points).await {
        error!("mqtt dispatch error on topic {}: {}", topic, e);
    }
    Ok(())
}

fn parse_device_id_from_topic(topic: &str) -> Result<&str, MqttReceiveError> {
    topic
        .split('/')
        .filter(|segment| !segment.is_empty())
        .nth_back(2)
        .ok_or_else(|| MqttReceiveError::Topic(topic.to_owned()))
}

fn parse_downlink_payload(raw: serde_json::Value) -> Result<Vec<DataPoint>, MqttReceiveError> {
    let raw_object = match raw {
        serde_json::Value::Object(map) => map,
        _ => return Err(MqttReceiveError::PayloadType),
    };
    let mut points: BTreeMap<u32, DataPoint> = BTreeMap::new();
    insert_points_from_map(&mut points, raw_object)?;

    Ok(points.into_values().collect())
}

fn insert_points_from_map(
    points: &mut BTreeMap<u32, DataPoint>,
    map: serde_json::Map<String, serde_json::Value>,
) -> Result<(), MqttReceiveError> {
    for (id, value) in map {
        let Ok(id) = id.parse::<u32>() else {
            continue;
        };
        let value = json_to_val(value).ok_or(MqttReceiveError::PointValue { point_id: id })?;
        points.insert(
            id,
            DataPoint {
                id,
                name: "",
                value,
            },
        );
    }

    Ok(())
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
                v.as_f64().map(|n| Val::F32(n as f32))
            }
        }
        serde_json::Value::Array(items) => items
            .into_iter()
            .map(json_to_val)
            .collect::<Option<Vec<_>>>()
            .map(Val::List),
        serde_json::Value::Null => None,
        serde_json::Value::String(v) => v.parse::<f32>().ok().map(Val::F32),
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
        Val::F32(value as f32)
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
        Val::F32(value as f32)
    }
}

async fn publish_routes_task(client: &AsyncClient, routes: &[MqttRoute]) {
    for route in routes {
        let dev = DevIdRef(route.device_id.as_str());
        for rule in &route.rules {
            let json = global_center()
                .with_snapshot(&dev, |snapshot| {
                    let mut map = serde_json::Map::with_capacity(rule.point_ids.len());
                    for it in snapshot.iter() {
                        if !rule.point_ids.contains(it.key()) {
                            continue;
                        }
                        if let Ok(v) = serde_json::to_value(&it.value) {
                            map.insert(it.key().to_string(), v);
                        }
                    }
                    if map.is_empty() {
                        None
                    } else {
                        serde_json::to_vec(&map).ok()
                    }
                })
                .flatten();
            if let Some(data) = json {
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
        assert_eq!(parse_device_id_from_topic("/pcs/0/yt").unwrap(), "pcs");
    }

    #[test]
    fn parses_device_id_from_four_segment_topic() {
        assert_eq!(parse_device_id_from_topic("/asw/pcs/0/yt").unwrap(), "pcs");
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
