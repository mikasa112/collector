//! 本地 IEC 104 模拟子站，点位与 `config/iec104/iec104_示例点表.xlsx` 对应。
//!
//! 运行：`cargo run -p collector-iec60870 --example slave -- [端口，默认 2404]`
//!
//! - 总召时上送全部遥信/遥测
//! - 每 3 秒模拟变化：断路器翻转（遥信）、电压/温度波动（遥测），以突发方式上送
//! - 收到遥控/遥调命令时在终端打印

use std::{thread, time::Duration};

use collector_iec60870::{DoublePointValue, PointValue, Slave, SlavePoint};

fn p(ioa: i32, value: PointValue) -> SlavePoint {
    SlavePoint { ca: 1, ioa, value }
}

fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .map(|s| s.parse().expect("端口必须是数字"))
        .unwrap_or(2404);

    let mut slave = Slave::new(
        port,
        vec![
            p(1, PointValue::Single(true)),
            p(2, PointValue::Single(false)),
            p(3, PointValue::Single(false)),
            p(16385, PointValue::Scaled(2200)),
            p(16386, PointValue::Scaled(1000)),
            p(16387, PointValue::Short(125000.0)),
            p(16388, PointValue::Scaled(650)),
            p(16389, PointValue::Short(50.0)),
            p(16390, PointValue::Scaled(0)),
            // 刀闸双点遥信，演示双点
            p(4, PointValue::Double(DoublePointValue::On)),
        ],
    )
    .expect("创建子站失败");
    slave.start();
    println!("模拟子站已启动，监听 0.0.0.0:{port}，Ctrl+C 退出");

    let mut tick = 0u32;
    loop {
        thread::sleep(Duration::from_secs(1));
        for cmd in slave.take_commands() {
            println!(
                "[收到命令] CA={} IOA={} select={} {:?}",
                cmd.ca, cmd.ioa, cmd.select, cmd.value
            );
        }
        tick += 1;
        if tick.is_multiple_of(3) {
            let n = tick / 3;
            slave.update(p(1, PointValue::Single(n.is_multiple_of(2))));
            slave.update(p(16385, PointValue::Scaled(2200 + (n % 10) as i32)));
            slave.update(p(16388, PointValue::Scaled(650 + (n % 5) as i32)));
            println!(
                "[突发上送] 断路器={} 电压原始值={}",
                n.is_multiple_of(2),
                2200 + n % 10
            );
        }
    }
}
