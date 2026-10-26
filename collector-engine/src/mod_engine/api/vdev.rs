use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, LazyLock, Mutex},
};

use collector_core::{
    center::{DataCenterError, data_center},
    core::point::{DataPoint, DownDataPoint, PointId, PointRef, Val},
};
use mlua::{Function, Lua, RegistryKey, Table, UserData, UserDataMethods, Value};
use tokio::{sync::mpsc, task::JoinHandle};

use super::dc::val_to_lua;
use crate::mod_engine::engine::EngineCmd;

/// 与 modbus_dev/can_dev 保持一致的"通讯状态"点位
const COMM_STATUS_ID: PointId = 0xFFFF;

/// DataPoint 的 key/name/unit 需要 `&'static str`。脚本传入的字符串去重后常驻，
/// 点位集合有限，泄漏量有上界；热更新重复注册同名点位不会重复增长。
fn intern(s: &str) -> &'static str {
    static POOL: LazyLock<Mutex<HashSet<&'static str>>> = LazyLock::new(Default::default);
    let mut pool = POOL.lock().unwrap();
    if let Some(&hit) = pool.get(s) {
        return hit;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// 脚本注册的设备：下发回调 + 把数据中心下行通道转发给引擎的任务
pub struct VDevEntry {
    pub callback: Option<RegistryKey>,
    task: JoinHandle<()>,
}

pub type VDevs = Arc<Mutex<HashMap<String, VDevEntry>>>;

/// 注销设备。仅当本脚本仍持有下行通道（任务未结束）时才 detach，
/// 避免设备已被 modbus_dev 等内置设备接管后误删对方的通道。
fn release(dev_id: &str, entry: VDevEntry) {
    if !entry.task.is_finished() {
        data_center().detach_downlink(dev_id);
    }
    entry.task.abort();
}

/// 引擎关闭时统一注销该脚本注册的所有设备
pub fn shutdown_all(vdevs: &VDevs) {
    let entries: Vec<_> = std::mem::take(&mut *vdevs.lock().unwrap())
        .into_iter()
        .collect();
    for (id, entry) in entries {
        release(&id, entry);
    }
}

/// 把一批下发点位转成 Lua 数组 `{ {id=|key=|name=, value=}, ... }`，供 on_write 回调使用
pub fn down_points_to_lua(lua: &Lua, points: &[DownDataPoint]) -> mlua::Result<Table> {
    let list = lua.create_table()?;
    for (i, p) in points.iter().enumerate() {
        let t = lua.create_table()?;
        match &p.point {
            PointRef::Id(id) => t.set("id", *id)?,
            PointRef::Key(k) => t.set("key", k.as_str())?,
            PointRef::Name(n) => t.set("name", n.as_str())?,
        }
        t.set("value", val_to_lua(lua, &p.value)?)?;
        list.set(i + 1, t)?;
    }
    Ok(list)
}

fn lua_to_val(value: Value, ty: Option<&str>) -> mlua::Result<Val> {
    let num = match &value {
        Value::Boolean(b) => return Ok(Val::U8(*b as u8)),
        Value::Integer(n) => *n as f64,
        Value::Number(n) => *n,
        other => {
            return Err(mlua::Error::runtime(format!(
                "点位 value 不支持 {} 类型",
                other.type_name()
            )));
        }
    };
    let ranged = |lo: f64, hi: f64| {
        (lo..=hi).contains(&num).then_some(num).ok_or_else(|| {
            mlua::Error::runtime(format!("点位值 {num} 超出 {} 范围", ty.unwrap_or("")))
        })
    };
    match ty.map(str::to_ascii_lowercase).as_deref() {
        None => Ok(match value {
            Value::Integer(n) if i32::try_from(n).is_ok() => Val::I32(n as i32),
            _ => Val::F64(num),
        }),
        Some("u8") => Ok(Val::U8(ranged(0.0, 255.0)? as u8)),
        Some("i8") => Ok(Val::I8(ranged(-128.0, 127.0)? as i8)),
        Some("u16") => Ok(Val::U16(ranged(0.0, 65535.0)? as u16)),
        Some("i16") => Ok(Val::I16(ranged(-32768.0, 32767.0)? as i16)),
        Some("u32") => Ok(Val::U32(ranged(0.0, u32::MAX as f64)? as u32)),
        Some("i32") => Ok(Val::I32(ranged(i32::MIN as f64, i32::MAX as f64)? as i32)),
        Some("f64") | Some("f32") => Ok(Val::F64(num)),
        Some(other) => Err(mlua::Error::runtime(format!("不支持的点位类型 {other}"))),
    }
}

fn table_to_point(t: &Table) -> mlua::Result<DataPoint> {
    let id: PointId = t.get("id")?;
    let key: String = t.get("key")?;
    let name: Option<String> = t.get("name")?;
    let unit: Option<String> = t.get("unit")?;
    let ty: Option<String> = t.get("type")?;
    let value = lua_to_val(t.get("value")?, ty.as_deref())?;
    Ok(DataPoint {
        id,
        key: intern(&key),
        name: intern(name.as_deref().unwrap_or(&key)),
        value,
        translator: None,
        bits: None,
        words: None,
        unit: unit.as_deref().map(intern),
        level: None,
    })
}

#[derive(Clone)]
struct VDevHandle {
    id: String,
    vdevs: VDevs,
}

impl UserData for VDevHandle {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        // dev:ingest({ {id=1, key="temp", name="温度", value=23.5, unit="℃", type="i16"}, ... })
        // type 可选：u8/i8/u16/i16/u32/i32/f64；缺省时整数按 i32、小数按 f64、boolean 按 u8
        methods.add_method("ingest", |_, this, points: Table| {
            let points = points
                .sequence_values::<Table>()
                .map(|t| table_to_point(&t?))
                .collect::<mlua::Result<Vec<_>>>()?;
            data_center().ingest(&this.id, points);
            Ok(())
        });

        // dev:set_comm(true)  有通讯；dev:set_comm(false) 通讯故障。与内置设备的 communicationStatus 点一致
        methods.add_method("set_comm", |_, this, online: bool| {
            data_center().ingest(
                &this.id,
                vec![DataPoint {
                    id: COMM_STATUS_ID,
                    key: "communicationStatus",
                    name: "通讯状态",
                    value: Val::U8(!online as u8),
                    translator: None,
                    bits: None,
                    words: None,
                    unit: None,
                    level: None,
                }],
            );
            Ok(())
        });

        methods.add_method("unregister", |_, this, ()| {
            if let Some(entry) = this.vdevs.lock().unwrap().remove(&this.id) {
                release(&this.id, entry);
            }
            Ok(())
        });
    }
}

/// 给 `dc` 表追加 `dc.register(dev_id, on_write?)`：把脚本注册为数据中心里的一个设备。
///
/// 注册后：
/// - `dev:ingest(points)` 上报的点位与内置设备完全一致，可被 dc.read/watch、上行、告警、历史等消费；
/// - 其他模块 `dc.dispatch(dev_id, ...)` 下发时，触发 `on_write(points)` 回调（在脚本 VM 内执行，可直接调用 modbus/dlt645 读写）。
///
/// dev_id 已被占用（如已存在同名 modbus_dev）时返回 `nil, err`，内置设备优先。
pub fn extend_dc_table(
    lua: &Lua,
    dc: &Table,
    cmd_tx: mpsc::UnboundedSender<EngineCmd>,
    vdevs: VDevs,
) -> mlua::Result<()> {
    dc.set(
        "register",
        lua.create_function(move |lua, (dev_id, on_write): (String, Option<Function>)| {
            let (tx, mut rx) = mpsc::channel::<Vec<DownDataPoint>>(16);
            match data_center().attach_downlink(&dev_id, tx) {
                Ok(()) => {}
                Err(DataCenterError::DevHasRegister(_)) => {
                    return Ok((Value::Nil, Some(format!("设备 {dev_id} 已被占用"))));
                }
                Err(e) => return Ok((Value::Nil, Some(e.to_string()))),
            }

            // 同一脚本重复注册同一 id（如热更新前未清理）：先释放旧条目
            if let Some(old) = vdevs.lock().unwrap().remove(&dev_id) {
                old.task.abort();
            }

            let task = {
                let cmd_tx = cmd_tx.clone();
                let dev_id = dev_id.clone();
                tokio::spawn(async move {
                    while let Some(points) = rx.recv().await {
                        let cmd = EngineCmd::DeviceWrite {
                            dev_id: dev_id.clone(),
                            points,
                        };
                        if cmd_tx.send(cmd).is_err() {
                            return;
                        }
                    }
                    // 通道被关闭：设备已被内置设备接管（modbus_dev 启动时会替换下行通道）
                    tracing::warn!("[mod] 脚本设备 {} 的下行通道已被接管，停止接收下发", dev_id);
                })
            };
            let callback = on_write.map(|f| lua.create_registry_value(f)).transpose()?;
            vdevs
                .lock()
                .unwrap()
                .insert(dev_id.clone(), VDevEntry { callback, task });

            let handle = VDevHandle {
                id: dev_id,
                vdevs: vdevs.clone(),
            };
            Ok((Value::UserData(lua.create_userdata(handle)?), None))
        })?,
    )
}
