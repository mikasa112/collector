use std::collections::HashMap;
use std::time::Duration;

use serde::Serialize;
use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, Value};

use crate::services::{ServiceError, ServiceResult};

pub struct NetworkService {}

#[derive(Debug, Serialize)]
pub struct WifiDev {
    ssid: String,
    strength: u8,
    frequency: u32,
}

// D-Bus a{sv} / a{sa{sv}}
type PropMap = HashMap<String, Value<'static>>;
type ConnSettings = HashMap<String, PropMap>;

const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const DEV_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const AP_IFACE: &str = "org.freedesktop.NetworkManager.AccessPoint";
const ACTIVE_CONN_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";

const NM_DEVICE_TYPE_WIFI: u32 = 2;
const NM_ACTIVE_STATE_ACTIVATED: u32 = 2;
const NM_ACTIVE_STATE_DEACTIVATING: u32 = 3;
const NM_ACTIVE_STATE_DEACTIVATED: u32 = 4;

fn val_str(s: impl Into<String>) -> Value<'static> {
    Value::from(s.into())
}

fn val_bytes(b: Vec<u8>) -> Value<'static> {
    Value::from(b)
}

fn build_conn_settings(ssid: &str, password: Option<&str>) -> ConnSettings {
    let mut settings = ConnSettings::new();

    let mut conn_map = PropMap::new();
    conn_map.insert("type".to_owned(), val_str("802-11-wireless"));
    conn_map.insert("id".to_owned(), val_str(ssid));
    settings.insert("connection".to_owned(), conn_map);

    let mut wifi_map = PropMap::new();
    wifi_map.insert("ssid".to_owned(), val_bytes(ssid.as_bytes().to_vec()));
    wifi_map.insert("mode".to_owned(), val_str("infrastructure"));
    settings.insert("802-11-wireless".to_owned(), wifi_map);

    if let Some(psk) = password.filter(|p| !p.is_empty()) {
        let mut sec_map = PropMap::new();
        sec_map.insert("key-mgmt".to_owned(), val_str("wpa-psk"));
        sec_map.insert("psk".to_owned(), val_str(psk));
        settings.insert("802-11-wireless-security".to_owned(), sec_map);
    }

    let mut ipv4_map = PropMap::new();
    ipv4_map.insert("method".to_owned(), val_str("auto"));
    settings.insert("ipv4".to_owned(), ipv4_map);

    let mut ipv6_map = PropMap::new();
    ipv6_map.insert("method".to_owned(), val_str("ignore"));
    settings.insert("ipv6".to_owned(), ipv6_map);

    settings
}

async fn find_wifi_device(conn: &Connection) -> ServiceResult<OwnedObjectPath> {
    let nm = zbus::Proxy::new(conn, NM_SERVICE, NM_PATH, NM_IFACE)
        .await
        .map_err(|e| ServiceError::InternalError(e.to_string()))?;

    let device_paths: Vec<OwnedObjectPath> = nm
        .call("GetDevices", &())
        .await
        .map_err(|e| ServiceError::InternalError(e.to_string()))?;

    for path in device_paths {
        let dev = zbus::Proxy::new(conn, NM_SERVICE, path.as_str(), DEV_IFACE)
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        let dev_type: u32 = dev
            .get_property("DeviceType")
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        if dev_type == NM_DEVICE_TYPE_WIFI {
            return Ok(path);
        }
    }

    Err(ServiceError::NotFound("未找到 WiFi 设备".to_string()))
}

impl NetworkService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {})
    }

    pub async fn scan(&self) -> ServiceResult<Vec<WifiDev>> {
        let conn = Connection::system()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        let wifi_path = find_wifi_device(&conn).await?;

        let wifi = zbus::Proxy::new(&conn, NM_SERVICE, wifi_path.as_str(), WIRELESS_IFACE)
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        let scan_opts = PropMap::new();
        let _: () = wifi
            .call("RequestScan", &(scan_opts,))
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        tokio::time::sleep(Duration::from_secs(3)).await;

        let ap_paths: Vec<OwnedObjectPath> = wifi
            .call("GetAccessPoints", &())
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        drop(wifi);

        let mut result = Vec::new();
        for ap_path in &ap_paths {
            let ap = zbus::Proxy::new(&conn, NM_SERVICE, ap_path.as_str(), AP_IFACE)
                .await
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;

            let ssid_bytes: Vec<u8> = ap
                .get_property("Ssid")
                .await
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;
            let strength: u8 = ap
                .get_property("Strength")
                .await
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;
            let frequency: u32 = ap
                .get_property("Frequency")
                .await
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;

            let ssid = String::from_utf8_lossy(&ssid_bytes).into_owned();
            result.push(WifiDev {
                ssid,
                strength,
                frequency,
            });
        }

        result.sort_by_key(|b| std::cmp::Reverse(b.strength));
        Ok(result)
    }

    pub async fn connect(&self, ssid: String, password: Option<String>) -> ServiceResult<()> {
        let conn = Connection::system()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        let wifi_path = find_wifi_device(&conn).await?;

        let nm = zbus::Proxy::new(&conn, NM_SERVICE, NM_PATH, NM_IFACE)
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        let settings = build_conn_settings(&ssid, password.as_deref());
        let root_path = zbus::zvariant::ObjectPath::try_from("/").unwrap();

        let (_, active_path): (OwnedObjectPath, OwnedObjectPath) = nm
            .call(
                "AddAndActivateConnection",
                &(settings, wifi_path.as_ref(), &root_path),
            )
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;

        drop(nm);

        // 轮询激活状态，最多等 30 秒
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err(ServiceError::InternalError("连接超时".to_string()));
            }

            let active =
                zbus::Proxy::new(&conn, NM_SERVICE, active_path.as_str(), ACTIVE_CONN_IFACE)
                    .await
                    .map_err(|e| ServiceError::InternalError(e.to_string()))?;

            let state: u32 = active
                .get_property("State")
                .await
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;

            match state {
                NM_ACTIVE_STATE_ACTIVATED => return Ok(()),
                NM_ACTIVE_STATE_DEACTIVATING | NM_ACTIVE_STATE_DEACTIVATED => {
                    return Err(ServiceError::InternalError(
                        "连接失败，请检查密码或信号".to_string(),
                    ));
                }
                _ => {}
            }

            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
}
