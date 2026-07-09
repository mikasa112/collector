use std::time::Duration;

use collector_core::{
    center::{self, SharedPointCenter},
    core::point::{DataPoint, Val},
};
use salvo::{
    Depot, Request, Response, handler,
    http::StatusError,
    prelude::WebSocketUpgrade,
    websocket::{Message, WebSocket},
};
use serde::{Deserialize, Serialize};
use tokio::time::{self, Instant};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DevQueryLang {
    En,
    Zh,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DevQueryParams {
    dev: String,
    lang: DevQueryLang,
}

#[handler]
pub async fn data_ws_handler(
    req: &mut Request,
    res: &mut Response,
    depot: &mut Depot,
) -> Result<(), StatusError> {
    let query = req
        .parse_queries::<DevQueryParams>()
        .map_err(|_| StatusError::bad_request())?;

    let center = depot
        .get::<SharedPointCenter>("center")
        .map_err(|_| StatusError::service_unavailable())?
        .clone();

    WebSocketUpgrade::new()
        .upgrade(req, res, move |mut ws| async move {
            handle_ws(&mut ws, center, query).await;
        })
        .await
}

#[derive(Debug, Clone, Serialize)]
struct Point<'a> {
    id: u32,
    key: &'static str,
    name: &'static str,
    value: &'a Val,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<&'static str>,
}

impl<'a> Point<'a> {
    fn from_data_point(data_point: &'a DataPoint, lang: DevQueryLang) -> Self {
        let name = match lang {
            DevQueryLang::Zh => data_point.name,
            DevQueryLang::En => data_point.translator.map_or(data_point.name, |t| t.en),
        };
        let mut status: Option<&'static str> = None;
        if let Some(sw) = data_point.status_word
            && let Ok(v) = u32::try_from(&data_point.value)
        {
            for (k, w) in sw.words.iter() {
                if *k == (v as u16) {
                    status = Some(match lang {
                        DevQueryLang::En => w.en,
                        DevQueryLang::Zh => w.zh,
                    });
                    break;
                }
            }
        }
        Point {
            id: data_point.id,
            key: data_point.key,
            name,
            value: &data_point.value,
            status,
            unit: data_point.unit,
        }
    }
}

const PUSH_THROTTLE: Duration = Duration::from_millis(500);

async fn push_points(ws: &mut WebSocket, data: &[DataPoint], lang: DevQueryLang) -> bool {
    let points = data
        .iter()
        .map(|p| Point::from_data_point(p, lang))
        .collect::<Vec<_>>();
    if let Ok(json) = serde_json::to_string(&points) {
        return ws.send(Message::text(json)).await.is_ok();
    }
    true
}

async fn handle_ws(ws: &mut WebSocket, center: SharedPointCenter, query: DevQueryParams) {
    let Some(mut rx) = center.subscribe(&query.dev) else {
        return;
    };

    // 建立连接后立即推送当前全量数据
    let initial = rx.borrow().clone();
    if !push_points(ws, &initial, query.lang).await {
        return;
    }

    let mut last_sent = Instant::now();
    let mut pending = false;

    loop {
        let deadline = last_sent + PUSH_THROTTLE;

        tokio::select! {
            result = rx.changed() => {
                if result.is_err() { break; }
                pending = true;
                // 抑制窗口已过：立即推送
                if Instant::now() >= deadline {
                    let data = rx.borrow().clone();
                    if !push_points(ws, &data, query.lang).await { break; }
                    last_sent = Instant::now();
                    pending = false;
                }
                // 否则等待下面的 sleep_until 分支到期后推送
            }

            // 抑制窗口到期，推送期间积压的最新值
            _ = time::sleep_until(deadline), if pending => {
                let data = rx.borrow().clone();
                if !push_points(ws, &data, query.lang).await { break; }
                last_sent = Instant::now();
                pending = false;
            }

            msg = ws.recv() => {
                match msg {
                    None => break,
                    Some(Ok(msg)) => {
                        if msg.is_close() { break; }
                        if msg.is_ping()
                            && ws.send(Message::pong(msg.as_bytes().to_vec())).await.is_err() {
                                break;
                            }
                    }
                    Some(Err(_)) => break,
                }
            }
        }
    }
}

#[derive(Debug, Serialize)]
struct HomeAcData {
    voltage: Option<f64>,
    current: Option<f64>,
    power: Option<f64>,
    frequency: Option<f64>,
}

impl HomeAcData {
    fn new(center: &SharedPointCenter) -> Self {
        //PCS 输入电压
        let pcs_v = center
            .read("pcs", 25)
            .and_then(|it| f64::try_from(it.value).ok());
        //PCS 输入功率
        let pcs_power = center
            .read("pcs", 24)
            .and_then(|it| f64::try_from(it.value).ok());
        //PCS 输入电流
        let pcs_current = center
            .read("pcs", 26)
            .and_then(|it| f64::try_from(it.value).ok());
        //电网频率
        let pcs_frequency = center
            .read("pcs", 7)
            .and_then(|it| f64::try_from(it.value).ok());
        HomeAcData {
            voltage: pcs_v,
            current: pcs_current,
            power: pcs_power,
            frequency: pcs_frequency,
        }
    }
}

#[derive(Debug, Serialize)]
struct HomeDcData {
    name: Option<String>,
    soc: Option<f64>,
    voltage: Option<f64>,
    highest_single_voltage: Option<f64>,
    lowest_single_voltage: Option<f64>,
    current: Option<f64>,
    power: Option<f64>,
    avg_temp: Option<f64>,
    highest_temp: Option<f64>,
    lowest_temp: Option<f64>,
}

impl HomeDcData {
    fn new(center: &SharedPointCenter) -> Self {
        let soc = center
            .read("bcu", 32)
            .and_then(|it| f64::try_from(it.value).ok());
        //bcu 单体累加和总压
        let voltage = center
            .read("bcu", 6)
            .and_then(|it| f64::try_from(it.value).ok());
        let highest_single_voltage = center
            .read("bcu", 9)
            .and_then(|it| f64::try_from(it.value).ok());
        let lowest_single_voltage = center
            .read("bcu", 13)
            .and_then(|it| f64::try_from(it.value).ok());
        // let current= center.read("bcu", )
        // let power = center.read("bcu", point_id)
        let avg_temp = center
            .read("bcu", 27)
            .and_then(|it| f64::try_from(it.value).ok());
        let highest_temp = center
            .read("bcu", 19)
            .and_then(|it| f64::try_from(it.value).ok());
        let lowest_temp = center
            .read("bcu", 23)
            .and_then(|it| f64::try_from(it.value).ok());
        Self {
            name: None,
            soc,
            voltage,
            highest_single_voltage,
            lowest_single_voltage,
            current: None,
            power: None,
            avg_temp,
            highest_temp,
            lowest_temp,
        }
    }
}

#[derive(Debug, Serialize)]
struct HomeCommonData {
    ac: HomeAcData,
    dc: HomeDcData,
}

#[handler]
pub async fn home_ws_handler(
    req: &mut Request,
    res: &mut Response,
    depot: &mut Depot,
) -> Result<(), StatusError> {
    let center = depot
        .get::<SharedPointCenter>("center")
        .map_err(|_| StatusError::service_unavailable())?
        .clone();
    WebSocketUpgrade::new()
        .upgrade(req, res, |mut ws| async move {
            handle_home_ws(&mut ws, center).await;
        })
        .await
}

// 首页业务数据待定，先保持连接骨架，收到 ping 回 pong
async fn handle_home_ws(ws: &mut WebSocket, center: SharedPointCenter) {
    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
    loop {}
}
