//! 用真实 PCS(瑞山 RS100)的 ICD 生成的模型搭一个本地模拟 IED, 再用本 crate 的客户端连上去。
//! 模型由 `build.rs` 调 `genmodel.jar` 从 `tests/data/INPOWER_MMS_1.5.icd` 现场生成(需要本机有 java)。
//!
//! 运行: `cargo test -p collector-iec61850 --features server --test pcs`
//!
//! 注意: 这里验证的是"协议实现与该 ICD 的模型自洽", 不能代替真机: 真机的 RCB 实际命名、
//! 是否校验 AP-Title 等仍需现场确认。

use std::{
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use calamine::{Data, Reader, Xlsx, open_workbook};
use collector_iec61850::{
    ConnectOptions, Connection, ControlOptions, DataSetLayout, Fc, ObjectRef, Report,
    ReportConfig, TRG_DATA_CHANGED, TRG_GI, TRG_INTEGRITY, TRG_QUALITY_CHANGED, TypeSpec, Value,
    ffi,
    testing::{CTL_APC, CTL_SPC, PCS_LD as LD, PcsSimulator as Pcs},
};

fn r(s: &str) -> ObjectRef {
    s.parse().unwrap()
}

fn connect(pcs: &Pcs) -> Connection {
    let conn = Connection::new(ConnectOptions::default()).unwrap();
    conn.connect("127.0.0.1", pcs.port).expect("连接模拟 PCS 失败");
    conn
}

/// 控制命令的处理函数在服务端的工作线程里异步执行, 采集端要靠回读确认, 这里同样轮询。
fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(3);
    while Instant::now() < end {
        if cond() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("等待超时: {what}");
}

fn recv(rx: &mpsc::Receiver<Report>) -> Report {
    rx.recv_timeout(Duration::from_secs(5)).expect("等待报告超时")
}

fn flatten(layout: &DataSetLayout, rep: &Report) -> Vec<(ObjectRef, Value)> {
    rep.entries.iter().flat_map(|e| layout.leaves(e.index, &e.value)).collect()
}

fn find<'a>(leaves: &'a [(ObjectRef, Value)], reference: &str) -> Option<&'a Value> {
    let want = r(reference);
    leaves.iter().find(|(k, _)| *k == want).map(|(_, v)| v)
}

/// 订阅一个带通配的 RCB, 返回 (选中的 RCB, 布局, 报告通道)。
fn subscribe(
    conn: &Connection,
    pattern: &str,
    cfg: &ReportConfig,
) -> (String, Arc<DataSetLayout>, mpsc::Receiver<Report>) {
    let cands = conn.resolve_rcbs(pattern).unwrap();
    let ds = conn.rcb_data_set(&cands[0]).unwrap();
    let layout = Arc::new(conn.prepare_data_set(&ds).unwrap());
    let (tx, rx) = mpsc::channel();
    let handler: Arc<dyn Fn(&Report) + Send + Sync> = Arc::new(move |rep| {
        let _ = tx.send(rep.clone());
    });
    let chosen = conn.subscribe_report_any(&cands, cfg, handler).expect("订阅失败");
    (chosen, layout, rx)
}

#[test]
fn model_matches_icd() {
    let pcs = Pcs::start();
    let conn = connect(&pcs);

    // LD 名 = IED 名 + ldInst; 点表里写后缀 MONT 即可。
    assert_eq!(conn.logical_devices().unwrap(), vec![LD.to_string()]);
    assert_eq!(conn.resolve_ld("MONT").unwrap(), LD);

    // LN 名带前缀: sta/ctl/set/dc/kw/dev + 类 + 实例号。
    let lns = conn.logical_nodes(LD).unwrap();
    for ln in ["LLN0", "staGGIO1", "staGGIO2", "ctlGAPC1", "setGAPC1", "setGAPC3", "MMXU1", "dcMMXN1", "kwGGIO1", "devGGIO4"] {
        assert!(lns.iter().any(|n| n == ln), "缺少 LN {ln}: {lns:?}");
    }
    let inds = conn.data_objects(&format!("{LD}/staGGIO1")).unwrap();
    assert!((1..=16).all(|i| inds.contains(&format!("Ind{i}"))), "{inds:?}");

    // 标准的带索引 RCB: 每个 RCB 16 个实例。
    let urcb = conn.resolve_rcbs("MONT/LLN0.RP.urcbAin*").unwrap();
    assert_eq!(urcb.len(), 16);
    assert_eq!(urcb[0], format!("{LD}/LLN0.RP.urcbAin01"));
    assert_eq!(urcb[15], format!("{LD}/LLN0.RP.urcbAin16"));
    assert_eq!(conn.resolve_rcbs("MONT/LLN0.BR.brcbDin*").unwrap().len(), 16);
    assert_eq!(conn.resolve_rcbs("MONT/LLN0.RP.urcbParameter*").unwrap().len(), 16);
    // 不同前缀不串: urcbAin* 不应匹配到 urcbParameter。
    assert!(urcb.iter().all(|n| n.contains("urcbAin")));

    // 每个 RCB 实例绑定各自的数据集。
    assert_eq!(conn.rcb_data_set(&urcb[0]).unwrap(), format!("{LD}/LLN0$dsAin"));
    assert_eq!(conn.rcb_data_set(&format!("{LD}/LLN0.BR.brcbDin01")).unwrap(), format!("{LD}/LLN0$dsDin"));
}

#[test]
fn data_set_layouts() {
    let pcs = Pcs::start();
    let conn = connect(&pcs);

    // dsDin: 28 个 DO 级成员(22 个 Ind + 6 个 SPCSO 的 ST 部分)
    let din = conn.prepare_data_set(&format!("{LD}/LLN0$dsDin")).unwrap();
    assert_eq!(din.len(), 28);
    assert_eq!(din.member(0).unwrap(), &r(&format!("{LD}/staGGIO1.Ind1[ST]")));
    assert_eq!(din.member(27).unwrap(), &r(&format!("{LD}/ctlGAPC1.SPCSO6[ST]")));

    // dsAin: 75 个 DO 级成员(MX), 含 PPV.phsAB 这类子对象
    let ain = conn.prepare_data_set(&format!("{LD}/LLN0$dsAin")).unwrap();
    assert_eq!(ain.len(), 75);
    assert!(ain.members().any(|m| *m == r(&format!("{LD}/MMXU1.PPV.phsAB[MX]"))));

    // dsParameter: 41 个叶子级成员(mxVal.f)
    let par = conn.prepare_data_set(&format!("{LD}/LLN0$dsParameter")).unwrap();
    assert_eq!(par.len(), 41);
    assert_eq!(par.member(0).unwrap(), &r(&format!("{LD}/setGAPC1.StrVal1.mxVal.f[MX]")));
    // 叶子成员展开后就是它自己
    let leaves = par.leaves(0, &Value::Float(1.5));
    assert_eq!(leaves, vec![(par.member(0).unwrap().clone(), Value::Float(1.5))]);
}

#[test]
fn analog_report_expands_do_members() {
    let pcs = Pcs::start();
    pcs.set_float("MMXU1.TotW.mag.f", 123.5);
    pcs.set_float("MMXU1.PPV.phsAB.cVal.mag.f", 380.25);
    let conn = connect(&pcs);

    let (rcb, layout, rx) = subscribe(&conn, "MONT/LLN0.RP.urcbAin*", &ReportConfig::default());
    assert!(rcb.ends_with("urcbAin01"), "{rcb}");

    // GI: 75 个成员全部上送, 展开后 MV 的 mag.f / q / t 都能按叶子引用取到。
    let gi = recv(&rx);
    assert_eq!(gi.entries.len(), 75, "{}", gi.entries.len());
    let leaves = flatten(&layout, &gi);
    assert_eq!(find(&leaves, &format!("{LD}/MMXU1.TotW.mag.f[MX]")), Some(&Value::Float(123.5)));
    assert_eq!(find(&leaves, &format!("{LD}/MMXU1.PPV.phsAB.cVal.mag.f[MX]")), Some(&Value::Float(380.25)));
    assert!(matches!(find(&leaves, &format!("{LD}/MMXU1.TotW.q[MX]")), Some(Value::Quality(_))));
    assert!(find(&leaves, &format!("{LD}/MMXU1.TotW.t[MX]")).is_some());
    // 每个 MV 成员展开 3 个叶子(mag.f/q/t), 共 75 × 3
    assert_eq!(leaves.len(), 75 * 3);

    // 数据变化: 只带变化的那个成员。
    pcs.set_float("MMXU1.TotW.mag.f", 200.0);
    let change = loop {
        let rep = recv(&rx);
        if find(&flatten(&layout, &rep), &format!("{LD}/MMXU1.TotW.mag.f[MX]")) == Some(&Value::Float(200.0)) {
            break rep;
        }
    };
    assert_eq!(change.entries.len(), 1, "只应上送变化的成员: {} 个", change.entries.len());
    assert!(change.timestamp_ms.is_some());
}

#[test]
fn status_report_and_second_client_gets_next_instance() {
    let pcs = Pcs::start();
    pcs.set_bool("staGGIO1.Ind3.stVal", false); // 静态模型的值会跨测试保留, 显式复位
    let a = connect(&pcs);
    let b = connect(&pcs);
    let cfg = ReportConfig { general_interrogation: false, ..Default::default() };

    let (rcb_a, layout, rx) = subscribe(&a, "MONT/LLN0.BR.brcbDin*", &cfg);
    let (rcb_b, _, _rx_b) = subscribe(&b, "MONT/LLN0.BR.brcbDin*", &cfg);
    assert!(rcb_a.ends_with("brcbDin01"), "{rcb_a}");
    assert!(rcb_b.ends_with("brcbDin02"), "第二个客户端应拿到下一个实例: {rcb_b}");

    pcs.set_bool("staGGIO1.Ind3.stVal", true);
    let rep = loop {
        let rep = recv(&rx);
        if find(&flatten(&layout, &rep), &format!("{LD}/staGGIO1.Ind3.stVal[ST]")) == Some(&Value::Bool(true)) {
            break rep;
        }
    };
    assert_eq!(rep.entries.len(), 1);
    assert_eq!(rep.entries[0].index, 2); // staGGIO1.Ind3 是 dsDin 的第 3 个成员
}

#[test]
fn buffered_report_replays_events_missed_while_disconnected() {
    let pcs = Pcs::start();
    pcs.set_bool("staGGIO1.Ind1.stVal", false); // 静态模型的值会跨测试保留, 显式复位
    pcs.set_bool("staGGIO1.Ind2.stVal", false);
    let cfg = ReportConfig { general_interrogation: false, ..Default::default() };

    // 第一次连接: 订阅后收到一次变化, 然后异常断开(不取消订阅)。
    let a = connect(&pcs);
    let (rcb, _layout, rx) = subscribe(&a, "MONT/LLN0.BR.brcbDin01", &cfg);
    pcs.set_bool("staGGIO1.Ind1.stVal", true);
    recv(&rx);
    drop(a);
    std::thread::sleep(Duration::from_millis(300));

    // 断线期间发生变化。
    pcs.set_bool("staGGIO1.Ind2.stVal", true);
    std::thread::sleep(Duration::from_millis(200));

    // 重连后订阅同一个 BRCB(不清缓冲), 应补发断线期间的事件。
    let b = connect(&pcs);
    let layout = Arc::new(b.prepare_data_set(&format!("{LD}/LLN0$dsDin")).unwrap());
    let (tx, rx) = mpsc::channel();
    b.subscribe_report(&rcb, &cfg, move |rep| {
        let _ = tx.send(rep.clone());
    })
    .unwrap();
    let mut seen = false;
    while let Ok(rep) = rx.recv_timeout(Duration::from_secs(3)) {
        if find(&flatten(&layout, &rep), &format!("{LD}/staGGIO1.Ind2.stVal[ST]")) == Some(&Value::Bool(true)) {
            seen = true;
            break;
        }
    }
    assert!(seen, "重连后没有补发断线期间的变化");
}

#[test]
fn control_spc_direct() {
    let pcs = Pcs::start();
    let conn = connect(&pcs);
    let target = conn.resolve_ref(&r("MONT/ctlGAPC1.SPCSO6")).unwrap().to_dotted();
    let feedback = format!("{LD}/ctlGAPC1.SPCSO6.stVal");
    let read = || conn.read(&feedback, Fc::St).unwrap();

    // 模型是静态全局的, 先把回读值置回初始状态, 不依赖其他测试的执行顺序。
    pcs.set_bool("ctlGAPC1.SPCSO6.stVal", false);
    assert_eq!(read(), Value::Bool(false));
    conn.control(&target, 1.0, &ControlOptions::default()).expect("遥控失败");
    wait_until("SPCSO6 回读为 true", || read() == Value::Bool(true));
    conn.control(&target, 0.0, &ControlOptions::default()).unwrap();
    wait_until("SPCSO6 回读为 false", || read() == Value::Bool(false));
    assert_eq!(pcs.calls(CTL_SPC), 2);
}

#[test]
fn control_apc_setpoint_with_readback_in_parameter_report() {
    let pcs = Pcs::start();
    let conn = connect(&pcs);

    // 订阅参数数据集, 遥调后应在报告里看到回读值。
    let (_, layout, rx) = subscribe(&conn, "MONT/LLN0.RP.urcbParameter*", &ReportConfig::default());
    recv(&rx); // GI

    let target = conn.resolve_ref(&r("MONT/setGAPC1.StrVal6")).unwrap().to_dotted();
    conn.control(&target, 1234.5, &ControlOptions::default()).expect("遥调失败");
    let readback = format!("{LD}/setGAPC1.StrVal6.mxVal.f");
    wait_until("StrVal6 回读为 1234.5", || conn.read(&readback, Fc::Mx).unwrap() == Value::Float(1234.5));
    assert_eq!(pcs.calls(CTL_APC), 1);

    let rep = loop {
        let rep = recv(&rx);
        if find(&flatten(&layout, &rep), &format!("{LD}/setGAPC1.StrVal6.mxVal.f[MX]")) == Some(&Value::Float(1234.5)) {
            break rep;
        }
    };
    assert_eq!(rep.entries.len(), 1);
    assert_eq!(rep.entries[0].index, 5); // StrVal6 是 dsParameter 的第 6 个成员
}

#[test]
fn control_status_only_is_rejected() {
    let pcs = Pcs::start();
    let conn = connect(&pcs);
    // LLN0.Mod 的 ctlModel 是 status-only, 没有 Oper, 不可控。
    assert!(conn.control(&format!("{LD}/LLN0.Mod"), 1.0, &ControlOptions::default()).is_err());
}

/// 这台 PCS 的控制对象都是 direct-with-normal-security: 服务端检查通过就立即回成功应答,
/// 真正执行是异步的, 客户端无法从应答得知设备是否执行。采集器必须靠回读点确认,
/// 所以点表要求每个遥控/遥调点配一个回读点。
#[test]
fn control_success_ack_does_not_mean_executed() {
    let pcs = Pcs::start();
    let conn = connect(&pcs);
    pcs.set_bool("ctlGAPC1.SPCSO1.stVal", false);

    // SPCSO1 在模拟器里没有挂处理函数, 相当于设备没有执行这条命令。
    conn.control(&format!("{LD}/ctlGAPC1.SPCSO1"), 1.0, &ControlOptions::default())
        .expect("normal security 下服务端不会拒绝");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(pcs.calls(CTL_SPC), 0);
    assert_eq!(
        conn.read(&format!("{LD}/ctlGAPC1.SPCSO1.stVal"), Fc::St).unwrap(),
        Value::Bool(false),
        "命令应答成功, 但回读值没变: 只有回读点能反映真实状态"
    );
}

#[test]
fn unused_constants_still_exported() {
    // 防止未使用告警: 这些常量是采集端订阅时会用到的触发条件。
    assert_ne!(TRG_DATA_CHANGED | TRG_QUALITY_CHANGED | TRG_INTEGRITY | TRG_GI, 0);
}

// ---------------------------------------------------------------------------------------------
// 反向校验 scripts/icd_to_iec61850_points.py 生成的点表: 同一份 ICD 既喂给脚本生成点表,
// 又喂给 genmodel 生成服务端模型, 两条独立的解析路径必须对得上。
// ---------------------------------------------------------------------------------------------

const POINTS_XLSX: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/pcs_points.xlsx");

fn text(cell: Option<&Data>) -> Option<String> {
    match cell {
        Some(Data::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

fn number(cell: Option<&Data>) -> Option<f64> {
    match cell {
        Some(Data::Float(f)) => Some(*f),
        Some(Data::Int(i)) => Some(*i as f64),
        _ => None,
    }
}

fn sheet_rows(name: &str) -> Vec<Vec<Data>> {
    let mut wb: Xlsx<_> = open_workbook(POINTS_XLSX).expect("打开点表失败");
    let range = wb.worksheet_range(name).unwrap_or_else(|e| panic!("读取 sheet {name} 失败: {e}"));
    range.rows().skip(1).map(|r| r.to_vec()).collect()
}

struct PointRow {
    key: String,
    reference: String,
    fc: Option<String>,
    kind: String,
    feedback: Option<String>,
}

fn load_points() -> Vec<PointRow> {
    sheet_rows("iec61850")
        .iter()
        .map(|r| PointRow {
            key: text(r.get(1)).expect("key 不能为空"),
            reference: text(r.get(3)).expect("ref 不能为空"),
            fc: text(r.get(4)),
            kind: text(r.get(5)).expect("type 不能为空"),
            feedback: text(r.get(15)),
        })
        .collect()
}

#[test]
fn generated_point_table_is_valid_against_the_model() {
    let pcs = Pcs::start();
    let conn = connect(&pcs);
    let points = load_points();
    let kinds = |k: &str| points.iter().filter(|p| p.kind == k).count();
    // 点表是带 --with-quality 生成的: 28 遥信 + 116 遥测 + 6 遥控 + 41 遥调 + 103 品质 + 103 时标
    assert_eq!(
        ["yx", "yc", "yk", "yt", "yq", "ytime"].map(kinds),
        [28, 116, 6, 41, 103, 103]
    );

    let by_key: std::collections::HashMap<&str, &PointRow> =
        points.iter().map(|p| (p.key.as_str(), p)).collect();
    let mut monitoring = Vec::new();

    for p in &points {
        let what = format!("{} ({} {})", p.key, p.kind, p.reference);
        let resolved = conn
            .resolve_ref(&p.reference.parse::<ObjectRef>().unwrap())
            .unwrap_or_else(|e| panic!("{what}: {e}"));

        match p.kind.as_str() {
            "yx" | "yc" | "yq" | "ytime" => {
                let fc: Fc = p.fc.as_deref().unwrap_or_else(|| panic!("{what}: 缺少 fc")).parse().unwrap();
                let v = conn
                    .read(&resolved.to_dotted(), fc)
                    .unwrap_or_else(|e| panic!("{what}: 读取失败: {e}"));
                let ok = matches!(
                    (p.kind.as_str(), &v),
                    ("yx", Value::Bool(_) | Value::Int(_) | Value::Uint(_) | Value::Dbpos(_))
                        | ("yc", Value::Float(_) | Value::Int(_) | Value::Uint(_))
                        | ("yq", Value::Quality(_))
                        | ("ytime", Value::UtcTime(_))
                );
                assert!(ok, "{what}: 值类型 {v:?} 与点类型不符");
                monitoring.push((resolved.with_fc(fc), p));
            }
            "yk" | "yt" => {
                assert!(p.fc.is_none(), "{what}: 控制点的 fc 应为空");
                // 控制模型已配置且可控
                let model = conn.read(&format!("{}.ctlModel", resolved.to_dotted()), Fc::Cf);
                assert!(matches!(model, Ok(Value::Int(1..=4))), "{what}: ctlModel = {model:?}");
                // ctlVal 的类型: 遥控是布尔, 遥调是 AnalogueValue 结构体
                let spec = conn.type_spec(&resolved.clone().with_fc(Fc::Co)).unwrap_or_else(|e| panic!("{what}: {e}"));
                let oper = match &spec {
                    TypeSpec::Struct(f) => f.iter().find(|(n, _)| n == "Oper").map(|(_, t)| t),
                    _ => None,
                }
                .unwrap_or_else(|| panic!("{what}: 没有 Oper: {spec:?}"));
                let ctl_val = match oper {
                    TypeSpec::Struct(f) => f.iter().find(|(n, _)| n == "ctlVal").map(|(_, t)| t),
                    _ => None,
                }
                .unwrap_or_else(|| panic!("{what}: Oper 里没有 ctlVal: {oper:?}"));
                match (p.kind.as_str(), ctl_val) {
                    ("yk", TypeSpec::Leaf(t)) => assert_eq!(*t, ffi::MmsType_MMS_BOOLEAN, "{what}"),
                    ("yt", TypeSpec::Struct(_)) => {}
                    other => panic!("{what}: ctlVal 类型与点类型不符: {other:?}"),
                }
                // 回读点存在, 是监视点, 并且就在这个控制对象之下
                let fb_key = p.feedback.as_deref().unwrap_or_else(|| panic!("{what}: 缺少 feedback"));
                let fb = by_key.get(fb_key).unwrap_or_else(|| panic!("{what}: feedback {fb_key} 不存在"));
                assert!(matches!(fb.kind.as_str(), "yx" | "yc"), "{what}: 回读点类型 {}", fb.kind);
                assert!(fb.reference.starts_with(&format!("{}.", p.reference)), "{what}: 回读点 {} 不在控制对象下", fb.reference);
            }
            other => panic!("{what}: 未知类型 {other}"),
        }
    }

    // source: 每个 RCB 能展开成实例, 数据集成员与点互相覆盖。
    let mut members_total = 0;
    for row in sheet_rows("source") {
        let kind = text(row.first()).unwrap();
        let pattern = text(row.get(1)).unwrap();
        let instances = conn.resolve_rcbs(&pattern).unwrap_or_else(|e| panic!("{pattern}: {e}"));
        assert_eq!(instances.len(), 16, "{pattern}: ICD 里 RptEnabled max=16");
        assert_eq!(kind == "brcb", pattern.contains(".BR."), "{pattern}: kind 与 BR/RP 不一致");
        assert!(number(row.get(4)).is_some(), "{pattern}: 缺少 intg_pd");

        let ds = conn.rcb_data_set(&instances[0]).unwrap();
        let layout = conn.prepare_data_set(&ds).unwrap();
        members_total += layout.len();
        for member in layout.members() {
            let fc = member.fc.expect("数据集成员必须带 FC");
            let hit = monitoring.iter().any(|(r, p)| member.covers(r) && r.fc == Some(fc) && p.kind != "yk");
            assert!(hit, "数据集 {ds} 的成员 {member} 没有任何点对应");
        }
    }
    assert_eq!(members_total, 28 + 75 + 41);

    // 反方向: 每个监视点都落在某个 RCB 的数据集里(这份 ICD 里没有需要轮询的点)。
    let covered: Vec<ObjectRef> = sheet_rows("source")
        .iter()
        .flat_map(|row| {
            let first = conn.resolve_rcbs(&text(row.get(1)).unwrap()).unwrap().remove(0);
            let ds = conn.rcb_data_set(&first).unwrap();
            conn.data_set_members(&ds).unwrap()
        })
        .collect();
    for (r, p) in &monitoring {
        assert!(
            covered.iter().any(|m| m.covers(r) && m.fc == r.fc),
            "点 {} ({r}) 不在任何 RCB 的数据集里",
            p.key
        );
    }
}
