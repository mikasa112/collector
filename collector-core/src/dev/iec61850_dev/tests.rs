//! 联调测试：用与真实 PCS 同模型的本地模拟 IED(`collector_iec61850::testing`)，让真实的
//! `Iec61850Dev` 通过 TCP 连接它，验证订阅报告、DO 级数据集展开、品质/时标、遥控遥调下发与读回、
//! 断线重连、RCB 被占用时降级为轮询。
//!
//! 模拟 IED 的模型是进程内唯一的全局静态模型, 值会跨测试保留, 所以每个测试都先显式复位自己用到的值。

use std::net::TcpListener;
use std::time::Duration;

use collector_iec61850::testing::{CTL_APC, CTL_SPC, PCS_LD, PcsSimulator};
use collector_iec61850::{ConnectOptions, Connection, ReportConfig};
use tokio::time::{Instant, sleep};

use super::Iec61850Dev;
use crate::center::data_center;
use crate::config::iec61850_conf::{Iec61850Configs, PointKind, build_configs};
use crate::config::{ComType, Device, DeviceConfig, IsoConfig, ProtocolConfigs};
use crate::core::point::{DownDataPoint, PointId, Val};
use crate::dev::{Lifecycle, LifecycleState};

const COMM_FAULT_ID: PointId = 0xFFFF;

fn manifest(rel: &str) -> String {
    format!("{}/../{rel}", env!("CARGO_MANIFEST_DIR"))
}

/// 默认的示例点表(191 个点, 不含品质/时标)。
fn example_configs() -> Iec61850Configs {
    build_configs(manifest("config/iec61850/iec61850_示例点表.xlsx")).unwrap()
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn device(id: &str, port: u16, interval_ms: u64, configs: Iec61850Configs) -> Iec61850Dev {
    let config = DeviceConfig {
        device_type: None,
        com_type: Some(ComType::IEC61850),
        register_file: None,
        interval: Some(interval_ms),
        timeout: Some(3000),
        request_interval: None,
        max_gap: None,
        ip: Some("127.0.0.1".into()),
        port: Some(port),
        slave: None,
        meter_address: None,
        serial_tty: None,
        baud_rate: None,
        data_bits: None,
        parity: None,
        stop_bits: None,
        interface: None,
        desc: None,
        // 取自真实 PCS 的 ICD; 模拟 IED 不校验, 这里只验证配置能一路传到连接参数
        iso: Some(IsoConfig {
            remote_ap_title: Some("1 3 9999 106".into()),
            remote_ae_qualifier: Some(33),
            remote_psel: Some("00 00 00 01".into()),
            remote_ssel: Some("00 01".into()),
            remote_tsel: Some("00 01".into()),
            ..Default::default()
        }),
    };
    Iec61850Dev::new(Device {
        id: Some(id.into()),
        desc: None,
        config,
        protocol_configs: Some(ProtocolConfigs::Iec61850(configs)),
    })
    .unwrap()
}

async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !f() {
        assert!(Instant::now() < deadline, "等待超时: {what}");
        sleep(Duration::from_millis(50)).await;
    }
}

fn value(dev: &str, key: &str) -> Option<Val> {
    data_center().read_by_key(dev, key).map(|p| p.value)
}

fn comm_fault(dev: &str) -> Option<Val> {
    data_center().read(dev, COMM_FAULT_ID).map(|p| p.value)
}

fn approx(v: Option<Val>, expected: f64) -> bool {
    v.and_then(|v| v.as_f64().ok())
        .is_some_and(|v| (v - expected).abs() < 1e-6 * expected.abs().max(1.0))
}

fn reset_values(sim: &PcsSimulator) {
    sim.set_float("MMXU1.TotW.mag.f", 123.5);
    sim.set_float("MMXU1.PPV.phsAB.cVal.mag.f", 380.25);
    sim.set_bool("staGGIO1.Ind1.stVal", false);
    sim.set_bool("ctlGAPC1.SPCSO6.stVal", false);
    sim.set_float("setGAPC1.StrVal6.mxVal.f", 0.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subscribes_receives_changes_and_dispatches_commands() {
    let dev_id = "iec61850-it-flow";
    let sim = PcsSimulator::start();
    reset_values(&sim);

    let mut dev = device(dev_id, sim.port, 0, example_configs());
    dev.init().unwrap();
    dev.start().await.unwrap();

    // 1. 连接 + 订阅 + 使能时的总召: DO 级成员展开到叶子, 按点表转换入库
    wait_until("总召数据入库", || {
        comm_fault(dev_id) == Some(Val::U8(0))
            && approx(value(dev_id, "MMXU1_TotW"), 123.5)
            && approx(value(dev_id, "MMXU1_PPV_phsAB"), 380.25)
            && value(dev_id, "staGGIO1_Ind1") == Some(Val::U8(0))
    })
    .await;
    assert_eq!(dev.state(), LifecycleState::Running);
    // 三个 RCB 的 GI 覆盖了点表里的全部点(28 遥信 + 116 遥测 + 通讯状态点; 控制点没有当前值)
    wait_until("全部监视点入库", || {
        data_center().read_all(dev_id).len() == 28 + 116 + 1
    })
    .await;

    // 2. 突发上送: 数据变化后无需再次总召
    sim.set_bool("staGGIO1.Ind1.stVal", true);
    sim.set_float("MMXU1.TotW.mag.f", 200.0);
    wait_until("变化上送", || {
        value(dev_id, "staGGIO1_Ind1") == Some(Val::U8(1))
            && approx(value(dev_id, "MMXU1_TotW"), 200.0)
    })
    .await;

    // 3. 遥控(布尔)和遥调(浮点)下发, 回读点随报告变化
    data_center()
        .dispatch(
            dev_id,
            vec![
                DownDataPoint::by_key("ctlGAPC1_SPCSO6_cmd".into(), Val::U8(1)),
                DownDataPoint::by_key("setGAPC1_StrVal6_cmd".into(), Val::F64(1234.5)),
            ],
        )
        .await
        .unwrap();
    wait_until("回读点变化", || {
        value(dev_id, "ctlGAPC1_SPCSO6") == Some(Val::U8(1))
            && approx(value(dev_id, "setGAPC1_StrVal6"), 1234.5)
    })
    .await;
    assert_eq!((sim.calls(CTL_SPC), sim.calls(CTL_APC)), (1, 1));

    // 按 id 下发, 并且能再把遥控置回去
    let spc_id = example_configs()
        .points
        .iter()
        .find(|p| p.key == "ctlGAPC1_SPCSO6_cmd".into())
        .unwrap()
        .id;
    data_center()
        .dispatch(dev_id, vec![DownDataPoint::by_id(spc_id, Val::U8(0))])
        .await
        .unwrap();
    wait_until("遥控置回", || {
        value(dev_id, "ctlGAPC1_SPCSO6") == Some(Val::U8(0))
    })
    .await;

    // 4. 非法下发被忽略, 不会产生命令
    let before = (sim.calls(CTL_SPC), sim.calls(CTL_APC));
    data_center()
        .dispatch(
            dev_id,
            vec![
                DownDataPoint::by_key("ctlGAPC1_SPCSO2_cmd".into(), Val::U8(1)), // 默认禁用的控制点
                DownDataPoint::by_key("MMXU1_TotW".into(), Val::F64(1.0)),       // 监视点不可下发
                DownDataPoint::by_id(9999, Val::U8(1)),                          // 点表里不存在
                DownDataPoint::by_key("setGAPC1_StrVal6_cmd".into(), Val::List(vec![])), // 值类型不对
            ],
        )
        .await
        .unwrap();
    sleep(Duration::from_millis(500)).await;
    assert_eq!(
        (sim.calls(CTL_SPC), sim.calls(CTL_APC)),
        before,
        "非法下发不应产生命令"
    );
    assert_eq!(before, (2, 1)); // 遥控下发 1 次 + 置回 1 次, 遥调下发 1 次

    // 5. 停止
    dev.stop().await.unwrap();
    assert_eq!(dev.state(), LifecycleState::Stopped);
    assert_eq!(comm_fault(dev_id), Some(Val::U8(1)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnects_after_ied_restart() {
    let dev_id = "iec61850-it-reconnect";
    let sim = PcsSimulator::start();
    let port = sim.port;
    reset_values(&sim);

    let mut dev = device(dev_id, port, 0, example_configs());
    dev.init().unwrap();
    dev.start().await.unwrap();
    wait_until("首次连接", || {
        dev.state() == LifecycleState::Running && approx(value(dev_id, "MMXU1_TotW"), 123.5)
    })
    .await;

    // IED 下线: 设备应上报通讯故障
    drop(sim);
    wait_until("上报通讯故障", || {
        comm_fault(dev_id) == Some(Val::U8(1))
    })
    .await;

    // IED 原地重启且值已变: 设备应自动重连、重新订阅并总召
    let sim = PcsSimulator::start_on(port);
    sim.set_float("MMXU1.TotW.mag.f", 77.0);
    wait_until("重连并重新订阅", || {
        comm_fault(dev_id) == Some(Val::U8(0)) && approx(value(dev_id, "MMXU1_TotW"), 77.0)
    })
    .await;
    assert_eq!(dev.state(), LifecycleState::Running);

    // 重新订阅后仍然能收到变化
    sim.set_float("MMXU1.TotW.mag.f", 88.0);
    wait_until("重连后变化上送", || {
        approx(value(dev_id, "MMXU1_TotW"), 88.0)
    })
    .await;

    dev.stop().await.unwrap();
    assert_eq!(dev.state(), LifecycleState::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unreachable_ied_reports_comm_fault_and_stops_promptly() {
    let dev_id = "iec61850-it-unreachable";
    let mut dev = device(dev_id, free_port(), 0, example_configs());
    dev.init().unwrap();
    dev.start().await.unwrap();

    wait_until("上报通讯故障", || {
        comm_fault(dev_id) == Some(Val::U8(1))
    })
    .await;
    assert_ne!(dev.state(), LifecycleState::Running);

    let started = Instant::now();
    dev.stop().await.unwrap();
    assert_eq!(dev.state(), LifecycleState::Stopped);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "停止用了 {:?}",
        started.elapsed()
    );
    assert_eq!(comm_fault(dev_id), Some(Val::U8(1)));
}

/// 报告控制块的所有实例都被其他客户端占用时, 它覆盖的点要降级为轮询, 而不是静默没有值。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_rcb_falls_back_to_polling() {
    let dev_id = "iec61850-it-busy";
    let sim = PcsSimulator::start();
    reset_values(&sim);

    // 16 个客户端占满 urcbAin01..16
    let quiet = ReportConfig {
        general_interrogation: false,
        ..Default::default()
    };
    let blockers: Vec<Connection> = (1..=16)
        .map(|i| {
            let conn = Connection::new(ConnectOptions::default()).unwrap();
            conn.connect("127.0.0.1", sim.port).unwrap();
            conn.subscribe_report(&format!("{PCS_LD}/LLN0.RP.urcbAin{i:02}"), &quiet, |_| {})
                .unwrap_or_else(|e| panic!("占用 urcbAin{i:02} 失败: {e}"));
            conn
        })
        .collect();

    // 设备周期 500ms: 被占用的 RCB 覆盖的遥测点靠轮询拿到值
    let mut dev = device(dev_id, sim.port, 500, example_configs());
    dev.init().unwrap();
    dev.start().await.unwrap();
    wait_until("轮询拿到遥测", || {
        approx(value(dev_id, "MMXU1_TotW"), 123.5)
    })
    .await;
    sim.set_float("MMXU1.TotW.mag.f", 250.0);
    wait_until("轮询跟上变化", || {
        approx(value(dev_id, "MMXU1_TotW"), 250.0)
    })
    .await;

    // 没被占用的 brcbDin 仍然走报告
    sim.set_bool("staGGIO1.Ind1.stVal", true);
    wait_until("遥信仍走报告", || {
        value(dev_id, "staGGIO1_Ind1") == Some(Val::U8(1))
    })
    .await;

    dev.stop().await.unwrap();
    drop(blockers);
}

/// 品质和时标是普通的监视点: 品质按标准位定义输出(有效性 1=无效, 细节位保持原位置)。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quality_and_time_points() {
    let dev_id = "iec61850-it-quality";
    let sim = PcsSimulator::start();
    reset_values(&sim);
    // libiec61850 的整数编码: 有效性 2=无效, 旧数据 = 128
    sim.set_quality("staGGIO1.Ind1.q", 2 | 128);
    sim.set_quality("staGGIO1.Ind2.q", 0);
    sim.set_time("MMXU1.TotW.t", 1_700_000_000_123);

    // 带 --with-quality 生成的点表: 品质/时标点默认禁用, 这里启用它们
    let mut configs =
        build_configs(manifest("collector-iec61850/tests/data/pcs_points.xlsx")).unwrap();
    let mut enabled = 0;
    for p in configs
        .points
        .iter_mut()
        .filter(|p| matches!(p.kind, PointKind::Yq | PointKind::Ytime))
    {
        p.enable = true;
        enabled += 1;
    }
    assert_eq!(enabled, 103 + 103);

    let mut dev = device(dev_id, sim.port, 0, configs);
    dev.init().unwrap();
    dev.start().await.unwrap();
    wait_until("品质与时标入库", || {
        value(dev_id, "staGGIO1_Ind1_q").is_some() && value(dev_id, "MMXU1_TotW_t").is_some()
    })
    .await;

    // 无效(标准编号 1) + 旧数据(bit7 = 128)
    assert_eq!(value(dev_id, "staGGIO1_Ind1_q"), Some(Val::U16(1 | 128)));
    assert_eq!(value(dev_id, "staGGIO1_Ind2_q"), Some(Val::U16(0)));
    assert_eq!(
        value(dev_id, "MMXU1_TotW_t"),
        Some(Val::F64(1_700_000_000_123.0))
    );

    dev.stop().await.unwrap();
}

#[test]
fn creating_a_device_validates_the_configuration() {
    let mk = |tweak: &dyn Fn(&mut DeviceConfig)| {
        let mut config = DeviceConfig {
            device_type: None,
            com_type: Some(ComType::IEC61850),
            register_file: None,
            interval: Some(0),
            timeout: Some(3000),
            request_interval: None,
            max_gap: None,
            ip: Some("127.0.0.1".into()),
            port: Some(102),
            slave: None,
            meter_address: None,
            serial_tty: None,
            baud_rate: None,
            data_bits: None,
            parity: None,
            stop_bits: None,
            interface: None,
            desc: None,
            iso: None,
        };
        tweak(&mut config);
        Iec61850Dev::new(Device {
            id: Some("iec61850-conf".into()),
            desc: None,
            config,
            protocol_configs: Some(ProtocolConfigs::Iec61850(Iec61850Configs::default())),
        })
        .err()
        .map(|e| e.to_string())
    };
    assert_eq!(mk(&|_| {}), None);
    assert!(mk(&|c| c.ip = None).unwrap().contains("IP"));
    assert!(
        mk(&|c| c.ip = Some("not-an-ip".into()))
            .unwrap()
            .contains("无效的IP")
    );
    assert!(mk(&|c| c.port = None).unwrap().contains("端口"));
    assert!(mk(&|c| c.interval = None).unwrap().contains("间隔"));
    assert!(mk(&|c| c.timeout = None).unwrap().contains("超时"));
    // ISO 参数在创建设备时就校验, 不用等到连接
    let bad_iso = |iso: IsoConfig| mk(&move |c| c.iso = Some(iso.clone())).unwrap();
    assert!(
        bad_iso(IsoConfig {
            remote_tsel: Some("zz".into()),
            ..Default::default()
        })
        .contains("ISO")
    );
    assert!(
        bad_iso(IsoConfig {
            remote_ap_title: Some("not an oid".into()),
            ..Default::default()
        })
        .contains("ISO")
    );

    // 别的协议的点表不能喂给 61850 设备
    let err = Iec61850Dev::new(Device {
        id: Some("x".into()),
        desc: None,
        config: DeviceConfig {
            iso: None,
            ..serde_json::from_str(r#"{"ip":"127.0.0.1","port":102,"interval":0,"timeout":1000}"#)
                .unwrap()
        },
        protocol_configs: Some(ProtocolConfigs::Iec104(vec![])),
    })
    .err()
    .unwrap();
    assert!(err.to_string().contains("不支持"), "{err}");
}
