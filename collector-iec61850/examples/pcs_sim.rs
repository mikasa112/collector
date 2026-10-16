//! 本地模拟 PCS(瑞山 RS100): 与真实设备同模型的 IEC 61850 服务端, 用于没有真机时联调采集器。
//!
//! ```text
//! cargo run -p collector-iec61850 --features server --example pcs_sim -- [端口, 默认 9102]
//! cargo run -p collector-cmd --bin colld -- -c config/iec61850/config_示例.json
//! ```
//!
//! 模型来自 `tests/data/INPOWER_MMS_1.5.icd`(构建时用 java 调 genmodel 生成)。启动后每秒让
//! 有功功率、一路遥信变化一次, 方便观察报告上送; `ctlGAPC1.SPCSO6`(遥控)和
//! `setGAPC1.StrVal6`(遥调)挂了处理函数, 下发后会把值写回对应的回读点。
//! 按 Ctrl-C 退出。

use std::{thread::sleep, time::Duration};

use collector_iec61850::testing::{CTL_APC, CTL_SPC, PcsSimulator};

fn main() {
    let port: u16 = std::env::args().nth(1).map_or(9102, |p| p.parse().expect("端口必须是数字"));
    let sim = PcsSimulator::start_on(port);
    sim.set_float("MMXU1.PPV.phsAB.cVal.mag.f", 380.0);
    sim.set_float("MMXU1.PPV.phsBC.cVal.mag.f", 381.5);
    sim.set_float("MMXU1.PPV.phsCA.cVal.mag.f", 379.5);
    sim.set_float("MMXU1.Hz.mag.f", 50.0);
    println!("模拟 PCS 已启动: 127.0.0.1:{port} (Ctrl-C 退出)");

    let (mut tick, mut last) = (0u32, (0, 0));
    loop {
        tick += 1;
        sim.set_float("MMXU1.TotW.mag.f", 100.0 + f32::from((tick % 20) as u8) * 5.0);
        sim.set_bool("staGGIO1.Ind3.stVal", tick % 2 == 0);
        let calls = (sim.calls(CTL_SPC), sim.calls(CTL_APC));
        if calls != last {
            println!("收到控制命令: 遥控 {} 次, 遥调 {} 次", calls.0, calls.1);
            last = calls;
        }
        sleep(Duration::from_secs(1));
    }
}
