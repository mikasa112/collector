//! 联调测试：起北向子站，用 `collector_iec60870::Client` 当上级主站，
//! 验证总召（多 CA）、突发上送、死区、时标、通讯故障品质、选择/执行两步遥控遥调。

use std::{
    net::TcpListener,
    sync::{Arc, Mutex},
    time::Duration,
};

use collector_iec60870::{Asdu, Client, InformationObjectValue, PointValue, SlaveOptions, quality};
use tokio::{
    sync::mpsc,
    time::{Instant, sleep},
};

use super::*;
use crate::config::north_iec104_conf::ServerPointKind;
use crate::core::point::{DataPoint, PointRef, Val};

const DEV: &str = "iec104-north-it";
const COT_SPONTANEOUS: u32 = 3;
const COT_ACT_CON: u32 = 7;
const COT_INTERROGATED: u32 = 20;

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn cfg(ca: i32, ioa: i32, kind: ServerPointKind, point_id: PointId) -> NorthIec104Config {
    NorthIec104Config {
        ca,
        ioa,
        kind,
        source: DEV.into(),
        point_id,
        scale: 1.0,
        offset: 0.0,
        deadband: 0.0,
        timestamp: false,
        name: format!("p{point_id}"),
    }
}

fn dp(id: PointId, value: Val) -> DataPoint {
    DataPoint {
        id,
        key: "k",
        name: "n",
        value,
        translator: None,
        bits: None,
        words: None,
        unit: None,
        level: None,
    }
}

async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !f() {
        assert!(Instant::now() < deadline, "等待超时: {what}");
        sleep(Duration::from_millis(50)).await;
    }
}

type Received = Arc<Mutex<Vec<Asdu>>>;

/// 找出最近收到的、指定 CA/IOA 的监视信息对象。
fn find(
    received: &Received,
    cot: u32,
    ca: i32,
    ioa: i32,
) -> Option<(InformationObjectValue, Option<u64>)> {
    received
        .lock()
        .unwrap()
        .iter()
        .rev()
        .filter(|a| a.cot == cot && a.ca == ca)
        .flat_map(|a| a.elements.iter())
        .find(|e| e.ioa == ioa)
        .map(|e| (e.value, e.timestamp_ms))
}

fn quality_of(v: InformationObjectValue) -> u8 {
    match v {
        InformationObjectValue::SinglePoint { quality, .. }
        | InformationObjectValue::DoublePoint { quality, .. }
        | InformationObjectValue::MeasuredNormalized { quality, .. }
        | InformationObjectValue::MeasuredScaled { quality, .. }
        | InformationObjectValue::MeasuredShort { quality, .. } => quality,
        InformationObjectValue::Unsupported { .. } => 0,
    }
}

/// 发送命令后等待命令确认，返回是否为否定确认。
async fn command_result(received: &Received, send: impl FnOnce()) -> bool {
    received.lock().unwrap().clear();
    send();
    wait_until("命令确认", || {
        received
            .lock()
            .unwrap()
            .iter()
            .any(|a| a.cot == COT_ACT_CON)
    })
    .await;
    received
        .lock()
        .unwrap()
        .iter()
        .find(|a| a.cot == COT_ACT_CON)
        .unwrap()
        .is_negative
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forward_and_commands() {
    use ServerPointKind::*;
    let center = data_center();
    center.ingest(
        DEV,
        vec![
            dp(1, Val::U8(0)),
            dp(2, Val::F64(220.0)),
            dp(COMM_STATUS_ID, Val::U8(0)),
        ],
    );
    let (tx, mut down_rx) = mpsc::channel(8);
    center.attach_downlink(DEV, tx).unwrap();

    let mut yc = cfg(1, 16385, YcScaled, 2);
    yc.scale = 0.1;
    yc.deadband = 1.0;
    let mut yx_ts = cfg(2, 1, Yx, 1); // 同一个点在 CA=2 下再暴露一次，带时标
    yx_ts.timestamp = true;
    let mut yt = cfg(1, 25089, YtShort, 11);
    yt.scale = 0.5;
    yt.offset = 10.0;
    let configs = vec![cfg(1, 1, Yx, 1), yc, yx_ts, cfg(1, 24577, Yk, 10), yt];

    let port = free_port();
    let mut options = SlaveOptions::new(port);
    options.local_address = Some("127.0.0.1".into());
    let server = Iec104Server {
        configs: Arc::new(configs),
        options,
        select_timeout: Duration::from_millis(300),
        require_select: true,
    };
    let shutdown = ShutdownManager::new();
    // 测试里没有 EMU 运行时，命令处理走 check_emu_gate=false 的路径
    tokio::spawn(server.start_inner(shutdown.clone(), false));

    let received: Received = Arc::default();
    let mut client = Client::new("127.0.0.1", port).unwrap();
    let sink = received.clone();
    client.set_asdu_received_handler(move |_, asdu| {
        sink.lock().unwrap().push(asdu);
        true
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while client.connect().is_err() {
        assert!(Instant::now() < deadline, "连接北向子站超时");
        sleep(Duration::from_millis(100)).await;
    }
    client.start_data_transfer();

    // 1. 总召 CA=1：只返回该 CA 的点，转发线程就绪后品质为良好
    let mut ok = false;
    for _ in 0..50 {
        received.lock().unwrap().clear();
        client.send_interrogation_command(1).unwrap();
        sleep(Duration::from_millis(200)).await;
        if let (Some((s, _)), Some((m, _))) = (
            find(&received, COT_INTERROGATED, 1, 1),
            find(&received, COT_INTERROGATED, 1, 16385),
        ) && quality_of(s) == quality::GOOD
            && quality_of(m) == quality::GOOD
        {
            assert!(matches!(
                m,
                InformationObjectValue::MeasuredScaled { value: 2200, .. }
            ));
            // CA=1 的总召不应带出 CA=2 的点
            assert!(find(&received, COT_INTERROGATED, 2, 1).is_none());
            ok = true;
            break;
        }
    }
    assert!(ok, "总召未拿到良好品质的数据");

    // 2. 突发上送 + 多 CA + 时标
    // 转发线程启动时会把初始值也突发上送一次，先等它落地再清空
    sleep(Duration::from_millis(500)).await;
    received.lock().unwrap().clear();
    center.ingest(DEV, vec![dp(1, Val::U8(1))]);
    wait_until("CA=1 遥信突发", || {
        find(&received, COT_SPONTANEOUS, 1, 1).is_some()
    })
    .await;
    wait_until("CA=2 遥信突发", || {
        find(&received, COT_SPONTANEOUS, 2, 1).is_some()
    })
    .await;
    let (v, ts) = find(&received, COT_SPONTANEOUS, 1, 1).unwrap();
    assert!(matches!(
        v,
        InformationObjectValue::SinglePoint { value: true, .. }
    ));
    assert!(ts.is_none());
    assert!(find(&received, COT_SPONTANEOUS, 2, 1).unwrap().1.is_some());

    // 3. 死区：变化 0.4 < 1.0 不上送；再到 221.5 才上送 2215
    center.ingest(DEV, vec![dp(2, Val::F64(220.4))]);
    sleep(Duration::from_millis(500)).await;
    assert!(find(&received, COT_SPONTANEOUS, 1, 16385).is_none());
    center.ingest(DEV, vec![dp(2, Val::F64(221.5))]);
    wait_until("遥测突发", || {
        find(&received, COT_SPONTANEOUS, 1, 16385).is_some()
    })
    .await;
    assert!(matches!(
        find(&received, COT_SPONTANEOUS, 1, 16385).unwrap().0,
        InformationObjectValue::MeasuredScaled { value: 2215, .. }
    ));

    // 4. 通讯故障 -> 品质无效；恢复 -> 良好
    received.lock().unwrap().clear();
    center.ingest(DEV, vec![dp(COMM_STATUS_ID, Val::U8(1))]);
    wait_until("故障品质", || {
        find(&received, COT_SPONTANEOUS, 1, 1)
            .is_some_and(|(v, _)| quality_of(v) == quality::INVALID)
    })
    .await;
    received.lock().unwrap().clear();
    center.ingest(DEV, vec![dp(COMM_STATUS_ID, Val::U8(0))]);
    wait_until("恢复品质", || {
        find(&received, COT_SPONTANEOUS, 1, 1).is_some_and(|(v, _)| quality_of(v) == quality::GOOD)
    })
    .await;

    // 5. 遥控：选择 -> 执行，成功下发
    assert!(
        !command_result(&received, || client
            .send_single_command(1, 24577, true, true, 0)
            .unwrap())
        .await
    );
    assert!(down_rx.try_recv().is_err(), "选择阶段不应下发");
    assert!(
        !command_result(&received, || client
            .send_single_command(1, 24577, true, false, 0)
            .unwrap())
        .await
    );
    let down = down_rx.recv().await.unwrap();
    assert_eq!(down.len(), 1);
    assert_eq!(down[0].point, PointRef::Id(10));
    assert_eq!(down[0].value, Val::U8(1));

    // 6. 没有选择直接执行（require_select）-> 否定确认，不下发
    assert!(
        command_result(&received, || client
            .send_single_command(1, 24577, true, false, 0)
            .unwrap())
        .await
    );
    // 选择超时后执行 -> 否定确认
    assert!(
        !command_result(&received, || client
            .send_single_command(1, 24577, false, true, 0)
            .unwrap())
        .await
    );
    sleep(Duration::from_millis(450)).await;
    assert!(
        command_result(&received, || client
            .send_single_command(1, 24577, false, false, 0)
            .unwrap())
        .await
    );
    // 未配置的控制点 -> 否定确认
    assert!(
        command_result(&received, || client
            .send_single_command(1, 30000, true, true, 0)
            .unwrap())
        .await
    );
    assert!(down_rx.try_recv().is_err());

    // 7. 遥调：raw 100 -> 工程值 100*0.5+10 = 60
    assert!(
        !command_result(&received, || client
            .send_setpoint_command_short(1, 25089, 100.0, true, 0)
            .unwrap())
        .await
    );
    assert!(
        !command_result(&received, || client
            .send_setpoint_command_short(1, 25089, 100.0, false, 0)
            .unwrap())
        .await
    );
    let down = down_rx.recv().await.unwrap();
    assert_eq!(down[0].point, PointRef::Id(11));
    assert_eq!(down[0].value, Val::F64(60.0));

    client.disconnect();
    shutdown.token().cancel();
    center.detach_downlink(DEV);
    let _ = PointValue::Single(false);
}
