//! 用 `tokio::io::duplex` 模拟 RS-485 总线上的电表，验证轮询、解码入库、多表、离线判定。

use std::collections::HashMap;
use std::time::Duration;

use collector_dlt645::{DataIdentifier, DiSpec, Frame, MeterAddress, spec_for};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream, duplex};
use tokio::sync::watch;
use tokio::time::{Instant, sleep};

use super::runner::{Dlt645Runner, Health, PollItem, build_plan};
use crate::center::data_center;
use crate::config::dlt645_conf::{COMM_STATUS_ID, Dlt645Config};
use crate::core::point::{PointId, Val};
use crate::dev::dev_config::Dlt645DeviceConfig;
use crate::dev::{LifecycleState, state::SharedState};

const DI_VOLTAGE: u32 = 0x0201_0100;
const DI_CURRENT: u32 = 0x0202_0100;
const DI_ENERGY: u32 = 0x0001_0000;

fn addr(s: &str) -> MeterAddress {
    MeterAddress::parse(s).unwrap()
}

fn point(id: PointId, key: &'static str, address: Option<&str>, di: u32) -> Dlt645Config {
    let di = DataIdentifier::from_u32(di);
    Dlt645Config {
        id,
        key,
        name: key,
        address: address.map(addr),
        spec: spec_for(di).unwrap(),
        scale: 1.0,
        offset: 0.0,
        unit: None,
        enable: true,
        trans: None,
        status_words: None,
        warn_bits: None,
        level: None,
    }
}

fn device_config(default_address: Option<&str>) -> Dlt645DeviceConfig {
    Dlt645DeviceConfig {
        serial_tty: "/dev/null".into(),
        baudrate: 2400,
        data_bits: 8,
        parity: "E".into(),
        stop_bits: 1,
        meter_address: default_address.map(addr),
        interval: Duration::ZERO,
        timeout: Duration::from_millis(80),
        request_interval: Duration::from_millis(1),
    }
}

/// 模拟总线上的电表：按 (地址, DI) 回复；表里没有的请求不回复(模拟离线)。
fn spawn_meters(mut bus: DuplexStream, table: Vec<(&'static str, u32, Vec<u8>)>) {
    let table: HashMap<(MeterAddress, u32), Vec<u8>> = table
        .into_iter()
        .map(|(a, di, data)| ((addr(a), di), data))
        .collect();
    tokio::spawn(async move {
        // 读数据请求: 4 个 FE + 16 字节帧
        let mut req = [0u8; 20];
        while bus.read_exact(&mut req).await.is_ok() {
            let Ok(frame) = Frame::decode(&req[4..]) else {
                continue;
            };
            let di = DataIdentifier::from_wire_data(&frame.data).unwrap();
            let Some(value) = table.get(&(frame.address, di.as_u32())) else {
                continue;
            };
            let mut data = di.to_wire_data().to_vec();
            data.extend_from_slice(value);
            let resp = Frame {
                address: frame.address,
                control: 0x91,
                data,
            }
            .encode()
            .unwrap();
            if bus.write_all(&resp).await.is_err() {
                break;
            }
        }
    });
}

struct Harness {
    stop_tx: watch::Sender<bool>,
    state: SharedState,
}

fn start(dev: &str, plan: Vec<PollItem>, default: Option<&str>, bus: DuplexStream) -> Harness {
    let (stop_tx, stop_rx) = watch::channel(false);
    let state = SharedState::new(LifecycleState::Ready);
    let runner = Dlt645Runner {
        id: dev.into(),
        config: device_config(default),
        plan,
        state: state.clone(),
        stop_rx,
    };
    tokio::spawn(runner.run_on(bus));
    Harness { stop_tx, state }
}

async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f() {
        assert!(Instant::now() < deadline, "等待超时: {what}");
        sleep(Duration::from_millis(10)).await;
    }
}

fn value(dev: &str, id: PointId) -> Option<Val> {
    data_center().read(dev, id).map(|p| p.value)
}

fn approx(v: Option<Val>, expected: f64) -> bool {
    v.and_then(|v| v.as_f64().ok())
        .is_some_and(|v| (v - expected).abs() < 1e-9)
}

#[tokio::test]
async fn polls_two_meters_on_one_bus_and_decodes_values() {
    let dev = "dlt645-it-two-meters";
    let (bus, meter_end) = duplex(1024);
    spawn_meters(
        meter_end,
        vec![
            ("000000000001", DI_VOLTAGE, vec![0x05, 0x22]), // 220.5 V
            ("000000000001", DI_CURRENT, vec![0x00, 0x15, 0x80]), // -1.500 A
            ("000000000002", DI_ENERGY, vec![0x67, 0x45, 0x23, 0x01]), // 12345.67 kWh
        ],
    );
    let plan = build_plan(
        vec![
            point(1, "voltage", Some("1"), DI_VOLTAGE),
            point(2, "current", Some("1"), DI_CURRENT),
            // 没写地址的点使用设备默认地址
            point(3, "energy", None, DI_ENERGY),
        ],
        Some(addr("2")),
    );
    assert_eq!(plan.len(), 3);

    let h = start(dev, plan, Some("2"), bus);
    wait_until("三个点入库", || {
        approx(value(dev, 1), 220.5)
            && approx(value(dev, 2), -1.5)
            && approx(value(dev, 3), 12345.67)
    })
    .await;
    wait_until("通讯状态恢复", || {
        value(dev, COMM_STATUS_ID) == Some(Val::U8(0))
    })
    .await;
    assert_eq!(h.state.load(), LifecycleState::Running);

    // 单位来自内置格式表，停止后通讯状态置为故障
    let dp = data_center().read(dev, 1).unwrap();
    assert_eq!(dp.key, "voltage");
    h.stop_tx.send(true).unwrap();
    wait_until("停止后置故障", || {
        h.state.load() == LifecycleState::Stopped && value(dev, COMM_STATUS_ID) == Some(Val::U8(1))
    })
    .await;
}

#[tokio::test]
async fn one_offline_meter_does_not_fault_the_bus() {
    let dev = "dlt645-it-one-offline";
    let (bus, meter_end) = duplex(1024);
    // 只有电表 1 在线
    spawn_meters(
        meter_end,
        vec![("000000000001", DI_VOLTAGE, vec![0x05, 0x22])],
    );
    let plan = build_plan(
        vec![
            point(1, "v1", Some("1"), DI_VOLTAGE),
            point(2, "v2", Some("2"), DI_VOLTAGE),
        ],
        None,
    );
    let h = start(dev, plan, None, bus);
    wait_until("在线电表入库", || approx(value(dev, 1), 220.5)).await;
    // 让离线电表至少失败超过阈值次数
    sleep(Duration::from_millis(80 * 5)).await;
    assert_eq!(value(dev, 2), None);
    assert_eq!(value(dev, COMM_STATUS_ID), Some(Val::U8(0)));
    h.stop_tx.send(true).unwrap();
}

#[tokio::test]
async fn all_meters_offline_keeps_comm_fault() {
    let dev = "dlt645-it-all-offline";
    let (bus, meter_end) = duplex(1024);
    spawn_meters(meter_end, vec![]);
    let plan = build_plan(vec![point(1, "v1", Some("1"), DI_VOLTAGE)], None);
    let h = start(dev, plan, None, bus);
    sleep(Duration::from_millis(80 * 6)).await;
    assert_eq!(value(dev, 1), None);
    assert_eq!(value(dev, COMM_STATUS_ID), Some(Val::U8(1)));
    assert_eq!(h.state.load(), LifecycleState::Running);
    h.stop_tx.send(true).unwrap();
}

#[test]
fn plan_skips_disabled_unaddressed_and_duplicate_points() {
    let mut disabled = point(1, "disabled", Some("1"), DI_VOLTAGE);
    disabled.enable = false;
    let plan = build_plan(
        vec![
            disabled,
            point(2, "ok", Some("1"), DI_VOLTAGE),
            // 同表同 DI 重复
            point(3, "dup", Some("000000000001"), DI_VOLTAGE),
            // 没有地址且没有默认地址
            point(4, "noaddr", None, DI_CURRENT),
            // 不同电表可以读同一个 DI
            point(5, "other", Some("2"), DI_VOLTAGE),
        ],
        None,
    );
    let keys: Vec<_> = plan.iter().map(|p| p.cfg.key).collect();
    assert_eq!(keys, ["ok", "other"]);
}

#[test]
fn health_tracks_fault_per_bus() {
    let spec = DiSpec::new(DataIdentifier::from_u32(DI_VOLTAGE), 2, 1, false, "V");
    let mk = |a: &str| PollItem {
        address: addr(a),
        cfg: Dlt645Config {
            spec,
            ..point(1, "k", Some(a), DI_VOLTAGE)
        },
    };
    let plan = [mk("1"), mk("2")];
    let (a, b) = (addr("1"), addr("2"));
    let mut h = Health::new(&plan);
    // 初始为故障，第一次有响应即恢复
    assert_eq!(h.record(a, true), Some(false));
    assert_eq!(h.record(a, true), None);
    // 电表 2 连续失败不影响总线状态
    for _ in 0..5 {
        assert_eq!(h.record(b, false), None);
    }
    // 电表 1 也连续失败后才判定总线故障
    assert_eq!(h.record(a, false), None);
    assert_eq!(h.record(a, false), None);
    assert_eq!(h.record(a, false), Some(true));
    // 任一电表恢复
    assert_eq!(h.record(b, true), Some(false));
}
