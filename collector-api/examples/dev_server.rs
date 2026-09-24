//! 前端本地开发用：只起 API/WS 服务并灌入几个假点位，不触碰任何硬件驱动、不读取 config/config.json。
//! 用法：cargo run -p collector-api --example dev_server
use std::collections::HashMap;

use collector_api::ApiApp;
use collector_core::center::data_center;
use collector_core::core::point::{DataPoint, Val, Word, Words};
use collector_core::shutdown::ShutdownManager;

fn main() {
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut word_map = HashMap::new();
        word_map.insert(
            0,
            Word {
                zh: "停止",
                en: "Stop",
            },
        );
        word_map.insert(
            1,
            Word {
                zh: "运行",
                en: "Run",
            },
        );
        let words: &'static Words = Box::leak(Box::new(Words(word_map)));

        data_center().ingest(
            "demo",
            vec![
                DataPoint {
                    id: 1,
                    key: "run_state",
                    name: "运行状态",
                    value: Val::U8(1),
                    translator: None,
                    bits: None,
                    words: Some(words),
                    unit: None,
                    level: None,
                },
                DataPoint {
                    id: 2,
                    key: "voltage",
                    name: "电压",
                    value: Val::F64(398.5),
                    translator: None,
                    bits: None,
                    words: None,
                    unit: Some("V"),
                    level: None,
                },
            ],
        );

        let shutdown = ShutdownManager::new();
        ApiApp::new("127.0.0.1".to_string(), 9091, None)
            .start(shutdown)
            .await;
    });
}
