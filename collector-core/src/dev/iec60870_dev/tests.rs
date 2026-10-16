//! 联调测试：用 `collector_iec60870::Slave` 起一个本地子站，让真实的 `Iec60870Dev`
//! 通过 TCP 连接它，验证总召、突发上送、遥控遥调下发、断线重连。

use std::net::TcpListener;
use std::time::Duration;

use collector_iec60870::{
    CommandValue, DoublePointValue, PointValue, ReceivedCommand, Slave, SlavePoint,
    StepCommandValue,
};
use tokio::time::{Instant, sleep};

use super::Iec60870Dev;
use crate::center::data_center;
use crate::config::iec104_conf::{Iec104Config, PointKind};
use crate::config::{ComType, Device, DeviceConfig, ProtocolConfigs};
use crate::core::point::{DownDataPoint, PointId, Val};
use crate::dev::{Lifecycle, LifecycleState};

const COMM_FAULT_ID: PointId = 0xFFFF;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn cfg(
    id: PointId,
    key: &'static str,
    ioa: i32,
    kind: PointKind,
    scale: f64,
    offset: f64,
) -> Iec104Config {
    Iec104Config {
        id,
        key,
        name: key,
        ca: 1,
        ioa,
        kind,
        scale,
        offset,
        unit: None,
        enable: true,
        trans: None,
        status_words: None,
        warn_bits: None,
        level: None,
    }
}

fn point_table() -> Vec<Iec104Config> {
    vec![
        cfg(1001, "breaker", 1, PointKind::Yx, 1.0, 0.0),
        cfg(1002, "switch", 2, PointKind::Yx, 1.0, 0.0),
        cfg(2001, "voltage", 16385, PointKind::Yc, 0.1, 0.0),
        cfg(2002, "power", 16386, PointKind::Yc, 1.0, 0.0),
        cfg(3001, "breakerCmd", 24577, PointKind::Yk, 1.0, 0.0),
        cfg(3002, "switchCmd", 24578, PointKind::YkDouble, 1.0, 0.0),
        cfg(4001, "powerSet", 25089, PointKind::YtShort, 0.5, 10.0),
        cfg(4002, "voltageSet", 25090, PointKind::YtScaled, 0.1, 0.0),
        cfg(4003, "tapChange", 25091, PointKind::YtStep, 1.0, 0.0),
        cfg(4004, "ratioSet", 25092, PointKind::YtNormalized, 1.0, 0.0),
    ]
}

fn slave_points(breaker: bool) -> Vec<SlavePoint> {
    let p = |ioa, value| SlavePoint { ca: 1, ioa, value };
    vec![
        p(1, PointValue::Single(breaker)),
        p(2, PointValue::Double(DoublePointValue::On)),
        p(16385, PointValue::Scaled(2201)),
        p(16386, PointValue::Short(220.1)),
    ]
}

fn device(id: &str, port: u16) -> Iec60870Dev {
    let config = DeviceConfig {
        device_type: None,
        com_type: Some(ComType::IEC104),
        register_file: None,
        interval: Some(0),
        timeout: Some(2000),
        request_interval: None,
        max_gap: None,
        ip: Some("127.0.0.1".into()),
        port: Some(port),
        slave: None,
        serial_tty: None,
        baud_rate: None,
        data_bits: None,
        parity: None,
        stop_bits: None,
        interface: None,
        desc: None,
        iso: None,
    };
    Iec60870Dev::new(Device {
        id: Some(id.into()),
        desc: None,
        config,
        protocol_configs: Some(ProtocolConfigs::Iec104(point_table())),
    })
    .unwrap()
}

async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "等待超时: {what}");
        sleep(Duration::from_millis(50)).await;
    }
}

fn value(dev: &str, id: PointId) -> Option<Val> {
    data_center().read(dev, id).map(|p| p.value)
}

fn approx(v: Option<Val>, expected: f64) -> bool {
    v.and_then(|v| v.as_f64().ok())
        .is_some_and(|v| (v - expected).abs() < 1e-9)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrogation_spontaneous_and_commands() {
    let dev_id = "iec104-it-flow";
    let port = free_port();
    let mut slave = Slave::new(port, slave_points(true)).unwrap();
    slave.start();

    let mut dev = device(dev_id, port);
    dev.init().unwrap();
    dev.start().await.unwrap();

    // 1. 连接 + 总召：遥信、遥测按点表转换入库
    wait_until("总召数据入库", || {
        value(dev_id, 1001) == Some(Val::U8(1))
            && value(dev_id, 1002) == Some(Val::U8(2))
            && approx(value(dev_id, 2001), 220.1)
            && approx(value(dev_id, 2002), 220.1)
    })
    .await;
    assert_eq!(dev.state(), LifecycleState::Running);
    assert_eq!(value(dev_id, COMM_FAULT_ID), Some(Val::U8(0)));

    // 2. 突发上送：子站变位后无需再次总召
    slave.update(SlavePoint {
        ca: 1,
        ioa: 1,
        value: PointValue::Single(false),
    });
    wait_until("突发变位入库", || {
        value(dev_id, 1001) == Some(Val::U8(0))
    })
    .await;

    // 3. 遥控遥调下发（含按 id / key 两种引用）
    let cmds = vec![
        DownDataPoint::by_id(3001, Val::U8(1)),
        DownDataPoint::by_id(3002, Val::U8(2)),
        DownDataPoint::by_id(4001, Val::F64(60.0)), // (60-10)/0.5 = 100
        DownDataPoint::by_id(4002, Val::F64(123.4)), // 123.4/0.1 = 1234
        DownDataPoint::by_key("tapChange".into(), Val::U8(2)),
        DownDataPoint::by_id(4004, Val::F64(0.5)),
    ];
    data_center().dispatch(dev_id, cmds).await.unwrap();

    let mut received: Vec<ReceivedCommand> = Vec::new();
    wait_until("子站收到6条命令", || {
        received.extend(slave.take_commands());
        received.len() >= 6
    })
    .await;
    let find = |ioa| received.iter().find(|c| c.ioa == ioa).copied().unwrap();
    assert!(received.iter().all(|c| c.ca == 1 && !c.select));
    assert_eq!(find(24577).value, CommandValue::Single(true));
    assert_eq!(
        find(24578).value,
        CommandValue::Double(DoublePointValue::On)
    );
    assert_eq!(find(25089).value, CommandValue::Short(100.0));
    assert_eq!(find(25090).value, CommandValue::Scaled(1234));
    assert_eq!(
        find(25091).value,
        CommandValue::Step(StepCommandValue::Higher)
    );
    assert_eq!(find(25092).value, CommandValue::Normalized(0.5));

    // 4. 非法下发被拒绝，不会产生命令
    let bad = vec![
        DownDataPoint::by_id(3002, Val::U8(0)), // 双点遥控只允许 1/2
        DownDataPoint::by_id(9999, Val::U8(1)), // 点表里不存在
        DownDataPoint::by_id(1001, Val::U8(1)), // 监视点不可下发
        DownDataPoint::by_id(4004, Val::F64(5.0)), // 归一化超出 -1~1
    ];
    data_center().dispatch(dev_id, bad).await.unwrap();
    sleep(Duration::from_millis(500)).await;
    assert!(slave.take_commands().is_empty());

    // 5. 停止
    dev.stop().await.unwrap();
    assert_eq!(dev.state(), LifecycleState::Stopped);
    assert_eq!(value(dev_id, COMM_FAULT_ID), Some(Val::U8(1)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnects_after_slave_restart() {
    let dev_id = "iec104-it-reconnect";
    let port = free_port();
    let mut slave = Slave::new(port, slave_points(true)).unwrap();
    slave.start();

    let mut dev = device(dev_id, port);
    dev.init().unwrap();
    dev.start().await.unwrap();
    wait_until("首次连接", || {
        dev.state() == LifecycleState::Running && value(dev_id, 1001) == Some(Val::U8(1))
    })
    .await;

    // 子站下线：设备应上报通讯故障
    drop(slave);
    wait_until("上报通讯故障", || {
        value(dev_id, COMM_FAULT_ID) == Some(Val::U8(1))
    })
    .await;

    // 子站重新上线且点值已变：设备应自动重连并重新总召
    let mut slave = Slave::new(port, slave_points(false)).unwrap();
    slave.start();
    wait_until("重连并重新总召", || {
        value(dev_id, COMM_FAULT_ID) == Some(Val::U8(0)) && value(dev_id, 1001) == Some(Val::U8(0))
    })
    .await;
    assert_eq!(dev.state(), LifecycleState::Running);

    dev.stop().await.unwrap();
    assert_eq!(dev.state(), LifecycleState::Stopped);
}
