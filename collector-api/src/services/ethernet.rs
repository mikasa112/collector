//! 有线网口的 IPv4 配置读取与修改（NetworkManager D-Bus）。
//!
//! 修改前先保存旧配置，应用后进入"待确认"状态：调用方在限定时间内（默认 60 秒）
//! 没有调 [`EthernetService::confirm`] 确认，就自动回滚到旧配置。这样即使改错 IP
//! 导致当前连接断开，也能自动恢复，无需到现场处理。

use std::{
    collections::HashMap,
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use parking_lot::Mutex;
use serde::Serialize;
use zbus::{
    Connection,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

use crate::services::{
    ServiceError, ServiceResult,
    ethernet_cfg::{Ipv4Method, StaticIpv4},
};

type PropMap = HashMap<String, Value<'static>>;
type ConnSettings = HashMap<String, PropMap>;
type RawSettings = HashMap<String, HashMap<String, OwnedValue>>;

const NM_SERVICE: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const NM_IFACE: &str = "org.freedesktop.NetworkManager";
const DEV_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const WIRED_IFACE: &str = "org.freedesktop.NetworkManager.Device.Wired";
const IP4_IFACE: &str = "org.freedesktop.NetworkManager.IP4Config";
const ACTIVE_CONN_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
const SETTINGS_CONN_IFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";

const NM_DEVICE_TYPE_ETHERNET: u32 = 1;
const ETHERNET_CONN_TYPE: &str = "802-3-ethernet";

/// 默认的确认等待时间
pub const DEFAULT_ROLLBACK_SECS: u64 = 60;

#[derive(Debug, Serialize)]
pub struct AddrInfo {
    address: String,
    prefix: u32,
}

#[derive(Debug, Default, Serialize)]
pub struct Ipv4Info {
    /// `auto`（DHCP）/ `manual`（静态）等，网口没有配置档案时为空
    method: Option<String>,
    addresses: Vec<AddrInfo>,
    gateway: Option<String>,
    dns: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct EthernetIface {
    name: String,
    mac: String,
    state: String,
    /// 是否插着网线
    carrier: bool,
    speed_mbps: Option<u32>,
    /// NetworkManager 连接配置档案名
    connection: Option<String>,
    /// 配置档案里保存的配置
    configured: Ipv4Info,
    /// 当前实际生效的地址（DHCP 取得的也在这里）
    current: Ipv4Info,
}

#[derive(Debug, Serialize)]
pub struct ApplyResult {
    /// 需在多少秒内调用确认接口，否则自动回滚
    pub rollback_secs: u64,
}

pub struct EthernetService {}

/// 一次尚未确认的修改，保存回滚所需的信息
struct Pending {
    generation: u64,
    name: String,
    device: OwnedObjectPath,
    profile: OwnedObjectPath,
    /// 修改前的配置；`None` 表示修改前网口没有配置档案（回滚时删除新建的档案）
    old: Option<ConnSettings>,
}

static PENDING: Mutex<Option<Pending>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// 串行化修改请求，避免两个请求同时检查"无待确认"后都去应用
static APPLYING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn dbus_err(e: impl std::fmt::Display) -> ServiceError {
    ServiceError::InternalError(e.to_string())
}

async fn proxy<'a>(
    conn: &'a Connection,
    path: &'a str,
    iface: &'a str,
) -> ServiceResult<zbus::Proxy<'a>> {
    zbus::Proxy::new(conn, NM_SERVICE, path, iface)
        .await
        .map_err(dbus_err)
}

fn root_path() -> ObjectPath<'static> {
    ObjectPath::try_from("/").unwrap()
}

fn is_none_path(p: &OwnedObjectPath) -> bool {
    p.as_str() == "/"
}

fn state_name(state: u32) -> &'static str {
    match state {
        10 => "unmanaged",
        20 => "unavailable",
        30 => "disconnected",
        40 => "prepare",
        50 => "config",
        60 => "need_auth",
        70 => "ip_config",
        80 => "ip_check",
        90 => "secondaries",
        100 => "activated",
        110 => "deactivating",
        120 => "failed",
        _ => "unknown",
    }
}

/// 物理有线网口：NM 类型为以太网，且在 sysfs 下有 `device` 链接
/// （veth、网桥、docker 等虚拟网卡没有）
async fn ethernet_devices(conn: &Connection) -> ServiceResult<Vec<(OwnedObjectPath, String)>> {
    let nm = proxy(conn, NM_PATH, NM_IFACE).await?;
    let paths: Vec<OwnedObjectPath> = nm.call("GetDevices", &()).await.map_err(dbus_err)?;
    let mut out = Vec::new();
    for path in paths {
        let dev = proxy(conn, path.as_str(), DEV_IFACE).await?;
        let dev_type: u32 = dev.get_property("DeviceType").await.map_err(dbus_err)?;
        if dev_type != NM_DEVICE_TYPE_ETHERNET {
            continue;
        }
        let name: String = dev.get_property("Interface").await.map_err(dbus_err)?;
        if Path::new(&format!("/sys/class/net/{name}/device")).exists() {
            out.push((path, name));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

fn ipv4_to_u32(ip: std::net::Ipv4Addr) -> u32 {
    // NM 的 au 字段按网络字节序存放，即内存中的八位组顺序
    u32::from_ne_bytes(ip.octets())
}

fn u32_to_ipv4(v: u32) -> std::net::Ipv4Addr {
    std::net::Ipv4Addr::from(v.to_ne_bytes())
}

fn owned_str(v: &OwnedValue) -> Option<String> {
    String::try_from(v.try_clone().ok()?).ok()
}

fn owned_u32(v: &OwnedValue) -> Option<u32> {
    u32::try_from(v.try_clone().ok()?).ok()
}

fn dict_list(v: &OwnedValue) -> Vec<HashMap<String, OwnedValue>> {
    v.try_clone()
        .ok()
        .and_then(|v| Vec::<HashMap<String, OwnedValue>>::try_from(v).ok())
        .unwrap_or_default()
}

/// `Value` 可能携带文件描述符，不能直接 `Clone`；网口配置里没有，逐项 `try_clone` 即可
fn clone_settings(s: &ConnSettings) -> Option<ConnSettings> {
    s.iter()
        .map(|(section, props)| {
            let props = props
                .iter()
                .map(|(k, v)| Some((k.clone(), v.try_clone().ok()?)))
                .collect::<Option<PropMap>>()?;
            Some((section.clone(), props))
        })
        .collect()
}

fn addr_infos(list: Vec<HashMap<String, OwnedValue>>) -> Vec<AddrInfo> {
    list.iter()
        .filter_map(|d| {
            let address = owned_str(d.get("address")?)?;
            let prefix = owned_u32(d.get("prefix")?)?;
            Some(AddrInfo { address, prefix })
        })
        .collect()
}

fn configured_ipv4(settings: &RawSettings) -> Ipv4Info {
    let Some(ipv4) = settings.get("ipv4") else {
        return Ipv4Info::default();
    };
    Ipv4Info {
        method: ipv4.get("method").and_then(owned_str),
        addresses: ipv4
            .get("address-data")
            .map(|v| addr_infos(dict_list(v)))
            .unwrap_or_default(),
        gateway: ipv4.get("gateway").and_then(owned_str),
        dns: ipv4
            .get("dns")
            .and_then(|v| Vec::<u32>::try_from(v.try_clone().ok()?).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|v| u32_to_ipv4(v).to_string())
            .collect(),
    }
}

async fn current_ipv4(conn: &Connection, dev: &zbus::Proxy<'_>) -> ServiceResult<Ipv4Info> {
    let cfg_path: OwnedObjectPath = dev.get_property("Ip4Config").await.map_err(dbus_err)?;
    if is_none_path(&cfg_path) {
        return Ok(Ipv4Info::default());
    }
    let cfg = proxy(conn, cfg_path.as_str(), IP4_IFACE).await?;
    let addresses: Vec<HashMap<String, OwnedValue>> =
        cfg.get_property("AddressData").await.map_err(dbus_err)?;
    let gateway: String = cfg.get_property("Gateway").await.map_err(dbus_err)?;
    let dns: Vec<HashMap<String, OwnedValue>> =
        cfg.get_property("NameserverData").await.map_err(dbus_err)?;
    Ok(Ipv4Info {
        method: None,
        addresses: addr_infos(addresses),
        gateway: Some(gateway).filter(|g| !g.is_empty()),
        dns: dns
            .iter()
            .filter_map(|d| owned_str(d.get("address")?))
            .collect(),
    })
}

async fn get_settings(conn: &Connection, profile: &OwnedObjectPath) -> ServiceResult<RawSettings> {
    let p = proxy(conn, profile.as_str(), SETTINGS_CONN_IFACE).await?;
    p.call("GetSettings", &()).await.map_err(dbus_err)
}

fn to_conn_settings(raw: RawSettings) -> ConnSettings {
    raw.into_iter()
        .map(|(section, props)| {
            let props = props
                .into_iter()
                .map(|(k, v)| (k, Value::from(v)))
                .collect::<PropMap>();
            (section, props)
        })
        .collect()
}

/// 网口当前使用的以太网配置档案：优先取已激活的，其次取可用档案里的第一个以太网档案
async fn find_profile(
    conn: &Connection,
    dev: &zbus::Proxy<'_>,
) -> ServiceResult<Option<OwnedObjectPath>> {
    let active: OwnedObjectPath = dev
        .get_property("ActiveConnection")
        .await
        .map_err(dbus_err)?;
    if !is_none_path(&active) {
        let a = proxy(conn, active.as_str(), ACTIVE_CONN_IFACE).await?;
        let profile: OwnedObjectPath = a.get_property("Connection").await.map_err(dbus_err)?;
        if !is_none_path(&profile) {
            return Ok(Some(profile));
        }
    }
    let available: Vec<OwnedObjectPath> = dev
        .get_property("AvailableConnections")
        .await
        .map_err(dbus_err)?;
    for profile in available {
        let settings = get_settings(conn, &profile).await?;
        let is_eth = settings
            .get("connection")
            .and_then(|c| c.get("type"))
            .and_then(owned_str)
            .is_some_and(|t| t == ETHERNET_CONN_TYPE);
        if is_eth {
            return Ok(Some(profile));
        }
    }
    Ok(None)
}

async fn read_iface(
    conn: &Connection,
    path: &OwnedObjectPath,
    name: String,
) -> ServiceResult<EthernetIface> {
    let dev = proxy(conn, path.as_str(), DEV_IFACE).await?;
    let wired = proxy(conn, path.as_str(), WIRED_IFACE).await?;
    let state: u32 = dev.get_property("State").await.map_err(dbus_err)?;
    let mac: String = wired.get_property("HwAddress").await.map_err(dbus_err)?;
    let carrier: bool = wired.get_property("Carrier").await.map_err(dbus_err)?;
    let speed: u32 = wired.get_property("Speed").await.map_err(dbus_err)?;

    let (connection, configured) = match find_profile(conn, &dev).await? {
        Some(profile) => {
            let settings = get_settings(conn, &profile).await?;
            let id = settings
                .get("connection")
                .and_then(|c| c.get("id"))
                .and_then(owned_str);
            (id, configured_ipv4(&settings))
        }
        None => (None, Ipv4Info::default()),
    };

    Ok(EthernetIface {
        name,
        mac,
        state: state_name(state).to_string(),
        carrier,
        speed_mbps: Some(speed).filter(|s| *s > 0),
        connection,
        configured,
        current: current_ipv4(conn, &dev).await?,
    })
}

/// 在已有 ipv4 配置上套用新的方式/静态参数，其余键（路由等）保持不变
fn apply_ipv4(settings: &mut ConnSettings, method: Ipv4Method, cfg: Option<&StaticIpv4>) {
    let ipv4 = settings.entry("ipv4".to_owned()).or_default();
    for key in [
        "addresses",
        "address-data",
        "gateway",
        "dns",
        "dns-data",
        "route-data",
    ] {
        // route-data 里可能带有旧网段的路由，静态配置时一并清掉；DHCP 下本来就不该有
        ipv4.remove(key);
    }
    match (method, cfg) {
        (Ipv4Method::Manual, Some(c)) => {
            ipv4.insert("method".to_owned(), Value::from("manual"));
            let addr: HashMap<String, Value<'static>> = HashMap::from([
                ("address".to_owned(), Value::from(c.address.to_string())),
                ("prefix".to_owned(), Value::from(c.prefix)),
            ]);
            ipv4.insert("address-data".to_owned(), Value::new(vec![addr]));
            if let Some(gw) = c.gateway {
                ipv4.insert("gateway".to_owned(), Value::from(gw.to_string()));
            }
            if !c.dns.is_empty() {
                let dns: Vec<u32> = c.dns.iter().map(|d| ipv4_to_u32(*d)).collect();
                ipv4.insert("dns".to_owned(), Value::new(dns));
            }
        }
        _ => {
            ipv4.insert("method".to_owned(), Value::from("auto"));
        }
    }
}

fn new_profile_settings(name: &str) -> ConnSettings {
    let mut settings = ConnSettings::new();
    settings.insert(
        "connection".to_owned(),
        PropMap::from([
            ("id".to_owned(), Value::from(format!("collector-{name}"))),
            ("type".to_owned(), Value::from(ETHERNET_CONN_TYPE)),
            ("interface-name".to_owned(), Value::from(name.to_string())),
            ("autoconnect".to_owned(), Value::from(true)),
        ]),
    );
    settings.insert(ETHERNET_CONN_TYPE.to_owned(), PropMap::new());
    settings.insert(
        "ipv6".to_owned(),
        PropMap::from([("method".to_owned(), Value::from("ignore"))]),
    );
    settings
}

async fn activate(
    conn: &Connection,
    profile: &OwnedObjectPath,
    device: &OwnedObjectPath,
) -> ServiceResult<()> {
    let nm = proxy(conn, NM_PATH, NM_IFACE).await?;
    let _: OwnedObjectPath = nm
        .call(
            "ActivateConnection",
            &(profile.as_ref(), device.as_ref(), &root_path()),
        )
        .await
        .map_err(dbus_err)?;
    Ok(())
}

async fn rollback(p: &Pending) -> ServiceResult<()> {
    let conn = Connection::system().await.map_err(dbus_err)?;
    match &p.old {
        Some(old) => {
            let profile = proxy(&conn, p.profile.as_str(), SETTINGS_CONN_IFACE).await?;
            let _: () = profile
                .call(
                    "Update",
                    &(clone_settings(old).ok_or_else(|| dbus_err("复制旧配置失败"))?,),
                )
                .await
                .map_err(dbus_err)?;
            activate(&conn, &p.profile, &p.device).await
        }
        None => {
            let profile = proxy(&conn, p.profile.as_str(), SETTINGS_CONN_IFACE).await?;
            let _: () = profile.call("Delete", &()).await.map_err(dbus_err)?;
            Ok(())
        }
    }
}

impl EthernetService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {})
    }

    pub async fn list(&self) -> ServiceResult<Vec<EthernetIface>> {
        let conn = Connection::system().await.map_err(dbus_err)?;
        let mut out = Vec::new();
        for (path, name) in ethernet_devices(&conn).await? {
            out.push(read_iface(&conn, &path, name).await?);
        }
        Ok(out)
    }

    /// 修改网口 IPv4 配置并立即应用，进入待确认状态。
    /// `cfg` 在 `Manual` 时必须给出；`Auto` 时忽略
    pub async fn apply(
        &self,
        name: &str,
        method: Ipv4Method,
        cfg: Option<StaticIpv4>,
        rollback_secs: u64,
    ) -> ServiceResult<ApplyResult> {
        if method == Ipv4Method::Manual && cfg.is_none() {
            return Err(ServiceError::InvalidParameter(
                "静态IP方式必须提供IP地址和子网前缀".to_string(),
            ));
        }
        let _guard = APPLYING.lock().await;
        if PENDING.lock().is_some() {
            return Err(ServiceError::BusinessLogic(
                "上一次网口修改尚未确认，请先确认，或等待自动回滚后再修改".to_string(),
            ));
        }

        let conn = Connection::system().await.map_err(dbus_err)?;
        let device = ethernet_devices(&conn)
            .await?
            .into_iter()
            .find(|(_, n)| n == name)
            .map(|(p, _)| p)
            .ok_or_else(|| ServiceError::NotFound(format!("未找到有线网口: {name}")))?;
        let dev = proxy(&conn, device.as_str(), DEV_IFACE).await?;

        let (profile, old) = match find_profile(&conn, &dev).await? {
            Some(profile) => {
                let old = to_conn_settings(get_settings(&conn, &profile).await?);
                let mut new = clone_settings(&old).ok_or_else(|| dbus_err("复制旧配置失败"))?;
                apply_ipv4(&mut new, method, cfg.as_ref());
                let p = proxy(&conn, profile.as_str(), SETTINGS_CONN_IFACE).await?;
                let _: () = p.call("Update", &(new,)).await.map_err(dbus_err)?;
                (profile, Some(old))
            }
            None => {
                let mut new = new_profile_settings(name);
                apply_ipv4(&mut new, method, cfg.as_ref());
                let settings = proxy(&conn, SETTINGS_PATH, SETTINGS_IFACE).await?;
                let profile: OwnedObjectPath = settings
                    .call("AddConnection", &(new,))
                    .await
                    .map_err(dbus_err)?;
                (profile, None)
            }
        };

        let generation = GENERATION.fetch_add(1, Ordering::Relaxed) + 1;
        let pending = Pending {
            generation,
            name: name.to_string(),
            device: device.clone(),
            profile: profile.clone(),
            old,
        };

        if let Err(err) = activate(&conn, &profile, &device).await {
            tracing::error!("[网口] {name} 应用新配置失败, 回滚: {err}");
            if let Err(e) = rollback(&pending).await {
                tracing::error!("[网口] {name} 回滚失败: {e}");
            }
            return Err(err);
        }

        tracing::warn!("[网口] {name} 配置已修改, {rollback_secs} 秒内未确认将自动回滚");
        *PENDING.lock() = Some(pending);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(rollback_secs)).await;
            let taken = {
                let mut guard = PENDING.lock();
                match guard.as_ref() {
                    Some(p) if p.generation == generation => guard.take(),
                    _ => None,
                }
            };
            if let Some(p) = taken {
                tracing::warn!("[网口] {} 修改未在时限内确认, 自动回滚", p.name);
                if let Err(err) = rollback(&p).await {
                    tracing::error!("[网口] {} 自动回滚失败: {}", p.name, err);
                }
            }
        });

        Ok(ApplyResult { rollback_secs })
    }

    /// 确认待确认的修改，取消自动回滚
    pub async fn confirm(&self) -> ServiceResult<()> {
        match PENDING.lock().take() {
            Some(p) => {
                tracing::info!("[网口] {} 修改已确认", p.name);
                Ok(())
            }
            None => Err(ServiceError::NotFound(
                "没有待确认的网口修改（可能已超时自动回滚）".to_string(),
            )),
        }
    }
}
