use std::collections::HashMap;
use std::time::Duration;

use dbus::arg::{RefArg, Variant};
use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
use networkmanager::NetworkManager;
use networkmanager::devices::{self, Wireless};
use serde::Serialize;

use crate::services::{ServiceError, ServiceResult};

pub struct NetworkService {}

#[derive(Debug, Serialize)]
pub struct WifiDev {
    ssid: String,
    strength: u8,
    frequency: u32,
}

// D-Bus a{sa{sv}} 类型
type PropMap = HashMap<String, Variant<Box<dyn RefArg + 'static>>>;
type ConnSettings = HashMap<String, PropMap>;

const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const DEV_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const ACTIVE_CONN_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";

// NM_DEVICE_TYPE_WIFI
const NM_DEVICE_TYPE_WIFI: u32 = 2;
// NM_ACTIVE_CONNECTION_STATE
const NM_ACTIVE_STATE_ACTIVATED: u32 = 2;
const NM_ACTIVE_STATE_DEACTIVATING: u32 = 3;
const NM_ACTIVE_STATE_DEACTIVATED: u32 = 4;

fn build_conn_settings(ssid: &str, password: Option<&str>) -> ConnSettings {
    let mut settings = ConnSettings::new();

    let mut conn_map: PropMap = HashMap::new();
    conn_map.insert(
        "type".to_owned(),
        Variant(Box::new("802-11-wireless".to_owned())),
    );
    conn_map.insert("id".to_owned(), Variant(Box::new(ssid.to_owned())));
    settings.insert("connection".to_owned(), conn_map);

    let mut wifi_map: PropMap = HashMap::new();
    wifi_map.insert(
        "ssid".to_owned(),
        Variant(Box::new(ssid.as_bytes().to_vec())),
    );
    wifi_map.insert(
        "mode".to_owned(),
        Variant(Box::new("infrastructure".to_owned())),
    );
    settings.insert("802-11-wireless".to_owned(), wifi_map);

    if let Some(psk) = password.filter(|p| !p.is_empty()) {
        let mut sec_map: PropMap = HashMap::new();
        sec_map.insert(
            "key-mgmt".to_owned(),
            Variant(Box::new("wpa-psk".to_owned())),
        );
        sec_map.insert("psk".to_owned(), Variant(Box::new(psk.to_owned())));
        settings.insert("802-11-wireless-security".to_owned(), sec_map);
    }

    let mut ipv4_map: PropMap = HashMap::new();
    ipv4_map.insert("method".to_owned(), Variant(Box::new("auto".to_owned())));
    settings.insert("ipv4".to_owned(), ipv4_map);

    let mut ipv6_map: PropMap = HashMap::new();
    ipv6_map.insert("method".to_owned(), Variant(Box::new("ignore".to_owned())));
    settings.insert("ipv6".to_owned(), ipv6_map);

    settings
}

impl NetworkService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {})
    }

    pub async fn scan(&self) -> ServiceResult<Vec<WifiDev>> {
        tokio::task::spawn_blocking(move || -> ServiceResult<Vec<WifiDev>> {
            let dbus_conn = dbus::blocking::Connection::new_system()
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;
            let nm = NetworkManager::new(&dbus_conn);
            let devices = nm
                .get_devices()
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;
            let mut wifi_dev = None;
            for dev in devices {
                match dev {
                    devices::Device::WiFi(wi_fi_device) => wifi_dev = Some(wi_fi_device),
                    _ => continue,
                }
            }
            let wifi = match wifi_dev {
                Some(dev) => dev,
                None => return Err(ServiceError::NotFound("No WiFi device found".to_string())),
            };
            let options = HashMap::new();
            wifi.request_scan(options)
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;
            std::thread::sleep(Duration::from_secs(3));
            let access_points = wifi
                .get_access_points()
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;
            let mut result = Vec::new();
            for it in access_points {
                result.push(WifiDev {
                    ssid: it
                        .ssid()
                        .map_err(|_| ServiceError::InternalError(String::from("ssid 错误")))?,
                    strength: it
                        .strength()
                        .map_err(|_| ServiceError::InternalError(String::from("strength 错误")))?,
                    frequency: it
                        .frequency()
                        .map_err(|_| ServiceError::InternalError(String::from("frequency 错误")))?,
                });
            }
            result.sort_by_key(|b| std::cmp::Reverse(b.strength));
            Ok(result)
        })
        .await
        .map_err(|e| ServiceError::InternalError(e.to_string()))?
    }

    pub async fn connect(&self, ssid: String, password: Option<String>) -> ServiceResult<()> {
        tokio::task::spawn_blocking(move || -> ServiceResult<()> {
            let conn = dbus::blocking::Connection::new_system()
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;

            let call_timeout = Duration::from_secs(5);
            let nm = conn.with_proxy(NM_SERVICE, NM_PATH, call_timeout);

            // 找到 WiFi 设备路径
            let (device_paths,): (Vec<dbus::Path>,) = nm
                .method_call(NM_IFACE, "GetDevices", ())
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;

            let mut wifi_path: Option<dbus::Path> = None;
            for path in device_paths {
                let dev = conn.with_proxy(NM_SERVICE, &path, call_timeout);
                let dev_type: u32 = dev
                    .get(DEV_IFACE, "DeviceType")
                    .map_err(|e| ServiceError::InternalError(e.to_string()))?;
                if dev_type == NM_DEVICE_TYPE_WIFI {
                    wifi_path = Some(path);
                    break;
                }
            }
            let wifi_path =
                wifi_path.ok_or_else(|| ServiceError::NotFound("未找到 WiFi 设备".to_string()))?;

            let settings = build_conn_settings(&ssid, password.as_deref());
            let specific_object = dbus::Path::new("/").unwrap();

            let (_, active_path): (dbus::Path, dbus::Path) = nm
                .method_call(
                    NM_IFACE,
                    "AddAndActivateConnection",
                    (settings, wifi_path, specific_object),
                )
                .map_err(|e| ServiceError::InternalError(e.to_string()))?;

            // 轮询激活状态，最多等 30 秒
            let deadline = std::time::Instant::now() + Duration::from_secs(30);
            loop {
                if std::time::Instant::now() >= deadline {
                    return Err(ServiceError::InternalError("连接超时".to_string()));
                }

                let active = conn.with_proxy(NM_SERVICE, &active_path, call_timeout);
                let state: u32 = active
                    .get(ACTIVE_CONN_IFACE, "State")
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

                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .await
        .map_err(|e| ServiceError::InternalError(e.to_string()))?
    }
}
