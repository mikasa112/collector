//! 连接真实 IED 的联调小工具: 连接、浏览模型、读值、订阅报告。
//!
//! ```text
//! probe <host> [--port 102] [--ap-title "1 3 9999 106" --ae-qualifier 33]
//!               [--psel "00 00 00 01"] [--ssel "00 01"] [--tsel "00 01"]
//!               [--browse] [--read <ref>:<FC>]... [--rcb <ref或带*通配>]... [--secs 30]
//! ```
//!
//! 例(对瑞山 PCS):
//! `probe 192.168.1.136 --ap-title "1 3 9999 106" --ae-qualifier 33 --browse \
//!    --read MONT/MMXU1.TotW.mag.f:MX --rcb "MONT/LLN0.RP.urcbAin*" --secs 20`

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use collector_iec61850::{
    ConnectOptions, Connection, Fc, IsoParams, ObjectRef, Report, ReportConfig, TRG_DATA_CHANGED,
    TRG_GI, TRG_INTEGRITY, TRG_QUALITY_CHANGED,
};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let host = args.next().ok_or_else(|| anyhow::anyhow!("用法: probe <host> [选项], 见源码注释"))?;
    let (mut port, mut secs) = (102u16, 30u64);
    let mut iso = IsoParams::default();
    let (mut browse, mut reads, mut rcbs) = (false, Vec::new(), Vec::new());
    while let Some(a) = args.next() {
        let mut val = || args.next().ok_or_else(|| anyhow::anyhow!("{a} 需要参数"));
        match a.as_str() {
            "--port" => port = val()?.parse()?,
            "--secs" => secs = val()?.parse()?,
            "--ap-title" => iso.remote_ap_title = Some(val()?),
            "--ae-qualifier" => iso.remote_ae_qualifier = Some(val()?.parse()?),
            "--psel" => iso.remote_psel = Some(val()?),
            "--ssel" => iso.remote_ssel = Some(val()?),
            "--tsel" => iso.remote_tsel = Some(val()?),
            "--browse" => browse = true,
            "--read" => reads.push(val()?),
            "--rcb" => rcbs.push(val()?),
            other => anyhow::bail!("未知参数 {other}"),
        }
    }

    let has_iso = iso != IsoParams::default();
    let conn = Connection::new(ConnectOptions { iso: has_iso.then_some(iso), ..Default::default() })?;
    conn.on_state_changed(|s| println!("[state] {s:?}"));
    conn.connect(&host, port)?;
    println!("已连接 {host}:{port}");

    let lds = conn.logical_devices()?;
    println!("LD: {lds:?}");
    if browse {
        for ld in &lds {
            for ln in conn.logical_nodes(ld)? {
                let dos = conn.data_objects(&format!("{ld}/{ln}")).unwrap_or_default();
                println!("  {ld}/{ln}: {} 个 DO {:?}", dos.len(), &dos[..dos.len().min(6)]);
            }
        }
    }

    for item in &reads {
        let (r, fc) = item.rsplit_once(':').ok_or_else(|| anyhow::anyhow!("--read 格式为 <ref>:<FC>"))?;
        let r = conn.resolve_ref(&r.parse::<ObjectRef>()?)?;
        let fc: Fc = fc.parse().map_err(|e: String| anyhow::anyhow!(e))?;
        println!("read {r} [{fc:?}] = {:?}", conn.read(&r.to_dotted(), fc));
    }

    for pattern in &rcbs {
        let candidates = conn.resolve_rcbs(pattern)?;
        println!("RCB 候选: {candidates:?}");
        let ds = conn.rcb_data_set(&candidates[0])?;
        // 必须在订阅前准备布局: 报告回调里不能再发同步请求。
        let layout = Arc::new(conn.prepare_data_set(&ds)?);
        println!("  数据集 {ds}: {} 个成员", layout.len());
        let cfg = ReportConfig {
            trg_ops: TRG_DATA_CHANGED | TRG_QUALITY_CHANGED | TRG_INTEGRITY | TRG_GI,
            ..Default::default()
        };
        let l = layout.clone();
        let chosen = conn.subscribe_report_any(
            &candidates,
            &cfg,
            Arc::new(move |rep: &Report| {
                let leaves: Vec<_> = rep.entries.iter().flat_map(|e| l.leaves(e.index, &e.value)).collect();
                println!(
                    "[report] {} seq={:?} {} 个成员 -> {} 个叶子, 前几项: {:?}",
                    rep.rcb_ref,
                    rep.seq_num,
                    rep.entries.len(),
                    leaves.len(),
                    leaves.iter().take(3).map(|(k, v)| format!("{k}={v:?}")).collect::<Vec<_>>()
                );
            }),
        )?;
        println!("  已订阅 {chosen}");
    }

    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end && conn.is_connected() {
        std::thread::sleep(Duration::from_millis(200));
    }
    println!("结束, 连接状态 {:?}", conn.state());
    Ok(())
}
