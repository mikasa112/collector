//! 进程内回环测试: 用 libiec61850 自带的服务端搭一个动态模型的测试 IED, 再用本 crate 的客户端连上去。
//! 运行: `cargo test -p collector-iec61850 --features server`

use std::{
    ffi::{CString, c_void},
    net::TcpListener,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicI64, Ordering},
        mpsc,
    },
    time::Duration,
};

use collector_iec61850::{
    ConnectOptions, Connection, ConnectionState, ControlOptions, Error, Fc, IsoParams, ObjectRef,
    Quality, REASON_DATA_CHANGE, Report, ReportConfig, Value, ffi,
};

const LD: &str = "testmodelSENSORS";

#[derive(Default)]
struct ControlState {
    spc_value: AtomicBool,
    spc_calls: AtomicI64,
    apc_value_x1000: AtomicI64,
    apc_calls: AtomicI64,
}

struct TestIed {
    server: ffi::IedServer,
    model: *mut ffi::IedModel,
    temp: *mut ffi::DataAttribute,
    temp_t: *mut ffi::DataAttribute,
    ind: *mut ffi::DataAttribute,
    port: u16,
    state: Arc<ControlState>,
}

unsafe impl Send for TestIed {}

fn c(s: &str) -> CString {
    CString::new(s).unwrap()
}

unsafe extern "C" fn spc_handler(
    _action: ffi::ControlAction,
    parameter: *mut c_void,
    ctl_val: *mut ffi::MmsValue,
    _test: bool,
) -> ffi::ControlHandlerResult {
    unsafe {
        let st = &*(parameter as *const ControlState);
        st.spc_value
            .store(ffi::MmsValue_getBoolean(ctl_val), Ordering::SeqCst);
        st.spc_calls.fetch_add(1, Ordering::SeqCst);
    }
    ffi::ControlHandlerResult_CONTROL_RESULT_OK
}

unsafe extern "C" fn apc_handler(
    _action: ffi::ControlAction,
    parameter: *mut c_void,
    ctl_val: *mut ffi::MmsValue,
    _test: bool,
) -> ffi::ControlHandlerResult {
    unsafe {
        let st = &*(parameter as *const ControlState);
        let v = if ffi::MmsValue_getType(ctl_val) == ffi::MmsType_MMS_STRUCTURE {
            ffi::MmsValue_getElement(ctl_val, 0)
        } else {
            ctl_val
        };
        let f = ffi::MmsValue_toFloat(v) as f64;
        st.apc_value_x1000.store((f * 1000.0).round() as i64, Ordering::SeqCst);
        st.apc_calls.fetch_add(1, Ordering::SeqCst);
    }
    ffi::ControlHandlerResult_CONTROL_RESULT_OK
}

impl TestIed {
    fn start() -> TestIed {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let state = Arc::new(ControlState::default());
        unsafe {
            let model = ffi::IedModel_create(c("testmodel").as_ptr());
            let ld = ffi::LogicalDevice_create(c("SENSORS").as_ptr(), model);
            let lln0 = ffi::LogicalNode_create(c("LLN0").as_ptr(), ld);
            ffi::CDC_ENS_create(c("Mod").as_ptr(), lln0 as *mut ffi::ModelNode, 0);
            ffi::CDC_ENS_create(c("Health").as_ptr(), lln0 as *mut ffi::ModelNode, 0);

            let ttmp1 = ffi::LogicalNode_create(c("TTMP1").as_ptr(), ld);
            let tmp = ffi::CDC_SAV_create(c("TmpSv").as_ptr(), ttmp1 as *mut ffi::ModelNode, 0, false);
            let temp = ffi::ModelNode_getChild(tmp as *mut ffi::ModelNode, c("instMag.f").as_ptr())
                as *mut ffi::DataAttribute;
            let temp_t = ffi::ModelNode_getChild(tmp as *mut ffi::ModelNode, c("t").as_ptr())
                as *mut ffi::DataAttribute;

            let ggio1 = ffi::LogicalNode_create(c("GGIO1").as_ptr(), ld);
            let ind1 = ffi::CDC_SPS_create(c("Ind1").as_ptr(), ggio1 as *mut ffi::ModelNode, 0);
            let ind = ffi::ModelNode_getChild(ind1 as *mut ffi::ModelNode, c("stVal").as_ptr())
                as *mut ffi::DataAttribute;
            let ind2 = ffi::CDC_SPS_create(c("Ind2").as_ptr(), ggio1 as *mut ffi::ModelNode, 0);
            let _ = ind2;
            let spc = ffi::CDC_SPC_create(
                c("SPCSO1").as_ptr(),
                ggio1 as *mut ffi::ModelNode,
                0,
                ffi::CDC_CTL_MODEL_DIRECT_NORMAL,
            );
            let apc = ffi::CDC_APC_create(
                c("AnOut1").as_ptr(),
                ggio1 as *mut ffi::ModelNode,
                0,
                ffi::CDC_CTL_MODEL_SBO_ENHANCED | ffi::CDC_CTL_MODEL_HAS_CANCEL,
                false,
            );

            let ds = ffi::DataSet_create(c("events").as_ptr(), lln0);
            // 与真实 PCS 一致: 既有 DO 级成员(整个 {stVal,q,t}), 也有叶子级成员。
            ffi::DataSetEntry_create(ds, c("TTMP1$MX$TmpSv").as_ptr(), -1, std::ptr::null());
            ffi::DataSetEntry_create(ds, c("GGIO1$ST$Ind1").as_ptr(), -1, std::ptr::null());
            ffi::DataSetEntry_create(ds, c("GGIO1$ST$Ind2$stVal").as_ptr(), -1, std::ptr::null());

            let opts = (ffi::RPT_OPT_SEQ_NUM | ffi::RPT_OPT_TIME_STAMP | ffi::RPT_OPT_REASON_FOR_INCLUSION) as u8;
            for name in ["events01", "events02"] {
                ffi::ReportControlBlock_create(
                    c(name).as_ptr(),
                    lln0,
                    c(name).as_ptr(),
                    false,
                    c("events").as_ptr(),
                    1,
                    ffi::TRG_OPT_DATA_CHANGED as u8,
                    opts,
                    50,
                    0,
                );
            }

            let server = ffi::IedServer_create(model);
            let param = Arc::as_ptr(&state) as *mut c_void;
            ffi::IedServer_setControlHandler(server, spc, Some(spc_handler), param);
            ffi::IedServer_setControlHandler(server, apc, Some(apc_handler), param);
            ffi::IedServer_start(server, port as i32);
            assert!(ffi::IedServer_isRunning(server), "测试 IED 启动失败");

            TestIed { server, model, temp, temp_t, ind, port, state }
        }
    }

    fn set_temp(&self, v: f32) {
        unsafe {
            ffi::IedServer_lockDataModel(self.server);
            ffi::IedServer_updateUTCTimeAttributeValue(self.server, self.temp_t, ffi::iec61850_Hal_getTimeInMs());
            ffi::IedServer_updateFloatAttributeValue(self.server, self.temp, v);
            ffi::IedServer_unlockDataModel(self.server);
        }
    }

    fn set_ind(&self, v: bool) {
        unsafe {
            ffi::IedServer_lockDataModel(self.server);
            ffi::IedServer_updateBooleanAttributeValue(self.server, self.ind, v);
            ffi::IedServer_unlockDataModel(self.server);
        }
    }
}

impl Drop for TestIed {
    fn drop(&mut self) {
        unsafe {
            ffi::IedServer_stop(self.server);
            ffi::IedServer_destroy(self.server);
            ffi::IedModel_destroy(self.model);
        }
    }
}

fn connect(ied: &TestIed) -> Connection {
    let conn = Connection::new(ConnectOptions::default()).unwrap();
    conn.connect("127.0.0.1", ied.port).expect("连接测试 IED 失败");
    assert!(conn.is_connected());
    conn
}

fn recv(rx: &mpsc::Receiver<Report>) -> Report {
    rx.recv_timeout(Duration::from_secs(5)).expect("等待报告超时")
}

#[test]
fn connect_browse_read() {
    let ied = TestIed::start();
    ied.set_temp(21.5);
    let conn = connect(&ied);

    assert_eq!(conn.logical_devices().unwrap(), vec![LD.to_string()]);
    let lns = conn.logical_nodes(LD).unwrap();
    assert!(lns.iter().any(|n| n == "TTMP1") && lns.iter().any(|n| n == "GGIO1"), "{lns:?}");
    let dos = conn.data_objects(&format!("{LD}/TTMP1")).unwrap();
    assert!(dos.iter().any(|n| n == "TmpSv"), "{dos:?}");
    let das = conn.data_attributes(&format!("{LD}/TTMP1.TmpSv")).unwrap();
    assert!(das.iter().any(|n| n.starts_with("instMag")), "{das:?}");

    let v = conn.read(&format!("{LD}/TTMP1.TmpSv.instMag.f"), Fc::Mx).unwrap();
    assert_eq!(v, Value::Float(21.5));

    // 整个 DO 读出来是结构体, 其中 q 应被识别为品质。
    let q = conn.read(&format!("{LD}/TTMP1.TmpSv.q"), Fc::Mx).unwrap();
    assert!(matches!(q, Value::Quality(q) if q.is_good()), "{q:?}");
    let t = conn.read(&format!("{LD}/TTMP1.TmpSv.t"), Fc::Mx).unwrap();
    assert!(matches!(t, Value::UtcTime(ms) if ms > 0), "{t:?}");

    let values = conn.read_data_set(&format!("{LD}/LLN0.events")).unwrap();
    assert_eq!(values.len(), 3);

    let err = conn.read(&format!("{LD}/TTMP1.NoSuch.f"), Fc::Mx).unwrap_err();
    assert!(matches!(err, Error::DataAccess { code: 10, .. }), "{err:?}");
}

fn r(s: &str) -> ObjectRef {
    s.parse().unwrap()
}

/// 把一份报告按布局展开成叶子: (叶子引用, 值)。
fn flatten(layout: &collector_iec61850::DataSetLayout, rep: &Report) -> Vec<(ObjectRef, Value)> {
    rep.entries
        .iter()
        .flat_map(|e| layout.leaves(e.index, &e.value))
        .collect()
}

fn find<'a>(leaves: &'a [(ObjectRef, Value)], reference: &str) -> Option<&'a Value> {
    let want = r(reference);
    leaves.iter().find(|(k, _)| *k == want).map(|(_, v)| v)
}

#[test]
fn report_expands_do_level_members() {
    let ied = TestIed::start();
    ied.set_temp(10.0);
    ied.set_ind(false);
    let conn = connect(&ied);

    let ds = format!("{LD}/LLN0.events");
    let layout = Arc::new(conn.prepare_data_set(&ds).unwrap());
    assert_eq!(layout.len(), 3);
    assert_eq!(layout.member(0).unwrap().to_mms().unwrap(), format!("{LD}/TTMP1$MX$TmpSv"));

    let (tx, rx) = mpsc::channel();
    let rcb = format!("{LD}/LLN0.RP.events01");
    conn.subscribe_report(&rcb, &ReportConfig::default(), move |rep| {
        let _ = tx.send(rep.clone());
    })
    .expect("订阅报告失败");

    // GI: 三个成员都带上; DO 级成员被展开到叶子, 叶子级成员保持原样。
    let gi = recv(&rx);
    assert_eq!(gi.entries.len(), 3, "{gi:?}");
    let leaves = flatten(&layout, &gi);
    assert_eq!(find(&leaves, &format!("{LD}/TTMP1.TmpSv.instMag.f[MX]")), Some(&Value::Float(10.0)));
    assert!(matches!(find(&leaves, &format!("{LD}/TTMP1.TmpSv.q[MX]")), Some(Value::Quality(q)) if *q == Quality::GOOD));
    assert!(matches!(find(&leaves, &format!("{LD}/TTMP1.TmpSv.t[MX]")), Some(Value::UtcTime(_))));
    assert_eq!(find(&leaves, &format!("{LD}/GGIO1.Ind1.stVal[ST]")), Some(&Value::Bool(false)));
    assert_eq!(find(&leaves, &format!("{LD}/GGIO1.Ind1.q[ST]")).map(|v| matches!(v, Value::Quality(_))), Some(true));
    assert_eq!(find(&leaves, &format!("{LD}/GGIO1.Ind2.stVal[ST]")), Some(&Value::Bool(false)));

    // 数据变化: 只带变化的成员, 原因是 data-change; 展开后仍能按叶子引用取到。
    ied.set_ind(true);
    let change = loop {
        let rep = recv(&rx);
        let l = flatten(&layout, &rep);
        if find(&l, &format!("{LD}/GGIO1.Ind1.stVal[ST]")) == Some(&Value::Bool(true)) {
            break rep;
        }
    };
    assert_eq!(change.entries.len(), 1, "{change:?}");
    assert_eq!(change.entries[0].index, 1);
    assert_eq!(change.entries[0].reason & REASON_DATA_CHANGE, REASON_DATA_CHANGE);
    assert!(change.timestamp_ms.is_some());
    // 报告自带的 data_ref 是 DO 级成员的 MMS 式引用, 与布局里的成员一致。
    assert_eq!(
        change.entries[0].data_ref.as_deref().map(|s| s.parse::<ObjectRef>().unwrap()),
        layout.member(1).cloned()
    );

    // 取消订阅后不再收到。
    conn.unsubscribe_report(&rcb).unwrap();
    while rx.try_recv().is_ok() {}
    ied.set_ind(false);
    assert!(rx.recv_timeout(Duration::from_millis(700)).is_err());
}

#[test]
fn rcb_wildcard_and_busy_detection() {
    let ied = TestIed::start();
    let a = connect(&ied);
    let b = connect(&ied);
    let c = connect(&ied);

    // LD 用后缀 `SENSORS`, 通配 `events*` 展开成两个实例。
    let cands = a.resolve_rcbs("SENSORS/LLN0.RP.events*").unwrap();
    assert_eq!(cands, vec![format!("{LD}/LLN0.RP.events01"), format!("{LD}/LLN0.RP.events02")]);
    assert_eq!(a.rcb_data_set(&cands[0]).unwrap(), format!("{LD}/LLN0$events"));
    // 不带通配: 原样返回(只展开 LD)。
    assert_eq!(a.resolve_rcbs("SENSORS/LLN0.RP.events02").unwrap(), vec![cands[1].clone()]);
    assert!(a.resolve_rcbs("SENSORS/LLN0.RP.nothere*").is_err());
    assert!(a.resolve_rcbs("SENSORS/LLN0.events").is_err());

    let noop: Arc<dyn Fn(&Report) + Send + Sync> = Arc::new(|_| {});
    let cfg = ReportConfig { general_interrogation: false, ..Default::default() };

    // 依次占用: a 拿 01, b 发现 01 被占用改拿 02, c 全部被占用。
    assert_eq!(a.subscribe_report_any(&cands, &cfg, noop.clone()).unwrap(), cands[0]);
    assert_eq!(b.subscribe_report_any(&cands, &cfg, noop.clone()).unwrap(), cands[1]);
    let err = c.subscribe_report_any(&cands, &cfg, noop.clone()).unwrap_err();
    assert!(matches!(err, Error::RcbBusy(_)), "{err:?}");

    // a 释放后, c 能拿到 01。
    a.unsubscribe_report(&cands[0]).unwrap();
    drop(a);
    assert_eq!(c.subscribe_report_any(&cands, &cfg, noop).unwrap(), cands[0]);
}

#[test]
fn ld_resolution() {
    let ied = TestIed::start();
    let conn = connect(&ied);
    assert_eq!(conn.resolve_ld("SENSORS").unwrap(), LD);
    assert_eq!(conn.resolve_ld(LD).unwrap(), LD);
    assert!(conn.resolve_ld("NOPE").is_err());
    let resolved = conn.resolve_ref(&r("SENSORS/GGIO1.Ind1.stVal[ST]")).unwrap();
    assert_eq!(resolved, r(&format!("{LD}/GGIO1.Ind1.stVal[ST]")));
    // 点表写短 LD 名时, 同样能读到值。
    assert_eq!(
        conn.read(&resolved.to_dotted(), Fc::St).unwrap(),
        Value::Bool(false)
    );
}

#[test]
fn iso_params_are_accepted_and_validated() {
    let ied = TestIed::start();
    // 取自真实 PCS 的 ICD。
    let iso = IsoParams {
        remote_ap_title: Some("1 3 9999 106".into()),
        remote_ae_qualifier: Some(33),
        remote_psel: Some("00 00 00 01".into()),
        remote_ssel: Some("00 01".into()),
        remote_tsel: Some("00 01".into()),
        ..Default::default()
    };
    let conn = Connection::new(ConnectOptions { iso: Some(iso), ..Default::default() }).unwrap();
    conn.connect("127.0.0.1", ied.port).expect("带 ISO 参数连接失败");
    assert!(conn.is_connected());

    // 非法参数在创建阶段就报错, 不会等到连接。
    for bad in [
        IsoParams { remote_tsel: Some("zz".into()), ..Default::default() },
        IsoParams { remote_ap_title: Some("not an oid".into()), ..Default::default() },
        IsoParams { remote_psel: Some("00".repeat(17)), ..Default::default() },
    ] {
        let err = Connection::new(ConnectOptions { iso: Some(bad), ..Default::default() }).err();
        assert!(matches!(err, Some(Error::InvalidArgument(_))), "{err:?}");
    }
    assert_eq!(IsoParams::parse_selector("00:01").unwrap(), vec![0, 1]);
    assert!(IsoParams::default().validate().is_ok());
    assert!(IsoParams { remote_tsel: Some("zz".into()), ..Default::default() }.validate().is_err());
}

#[test]
fn control_direct_and_sbo() {
    let ied = TestIed::start();
    let conn = connect(&ied);
    let opts = ControlOptions::default();

    // SPC, direct-normal: ctlVal 为布尔。
    conn.control(&format!("{LD}/GGIO1.SPCSO1"), 1.0, &opts).expect("SPC 控制失败");
    assert_eq!(ied.state.spc_calls.load(Ordering::SeqCst), 1);
    assert!(ied.state.spc_value.load(Ordering::SeqCst));
    conn.control(&format!("{LD}/GGIO1.SPCSO1"), 0.0, &opts).unwrap();
    assert!(!ied.state.spc_value.load(Ordering::SeqCst));

    // APC, SBO-enhanced: selectWithValue + operate, ctlVal 为浮点。
    conn.control(&format!("{LD}/GGIO1.AnOut1"), 12.5, &opts).expect("APC 控制失败");
    assert_eq!(ied.state.apc_calls.load(Ordering::SeqCst), 1);
    assert_eq!(ied.state.apc_value_x1000.load(Ordering::SeqCst), 12_500);

    // 不存在的控制对象。
    let err = conn.control(&format!("{LD}/GGIO1.Nope"), 1.0, &opts).unwrap_err();
    assert!(matches!(err, Error::Client { .. }), "{err:?}");
}

#[test]
fn close_and_state_callback() {
    let ied = TestIed::start();
    let conn = Connection::new(ConnectOptions::default()).unwrap();
    let (tx, rx) = mpsc::channel();
    conn.on_state_changed(move |s| {
        let _ = tx.send(s);
    });
    conn.connect("127.0.0.1", ied.port).unwrap();
    conn.close();
    let mut seen = Vec::new();
    while let Ok(s) = rx.recv_timeout(Duration::from_secs(2)) {
        seen.push(s);
        if s == ConnectionState::Closed {
            break;
        }
    }
    assert!(seen.contains(&ConnectionState::Connected), "{seen:?}");
    assert_eq!(seen.last(), Some(&ConnectionState::Closed), "{seen:?}");
    assert!(conn.read(&format!("{LD}/TTMP1.TmpSv.instMag.f"), Fc::Mx).unwrap_err().is_connection_lost());
}
