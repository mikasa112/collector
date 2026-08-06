use std::time::Duration;

use collector_core::{
    center::SharedPointCenter,
    core::point::{DataPoint, PointId, Val, Words},
    utils::taos::{TaosDbError, query_rows},
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
    words: Option<&'static Words>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<&'static str>,
}

impl<'a> Point<'a> {
    fn from_data_point(data_point: &'a DataPoint, lang: DevQueryLang) -> Self {
        let name = match lang {
            DevQueryLang::Zh => data_point.name,
            DevQueryLang::En => data_point.translator.map_or(data_point.name, |t| t.en),
        };
        Point {
            id: data_point.id,
            key: data_point.key,
            name,
            value: &data_point.value,
            words: data_point.words,
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
        let pcs_va = center
            .read("pcs", 1)
            .and_then(|it| f64::try_from(it.value).ok());

        let pcs_vb = center
            .read("pcs", 2)
            .and_then(|it| f64::try_from(it.value).ok());

        let pcs_vc = center
            .read("pcs", 3)
            .and_then(|it| f64::try_from(it.value).ok());
        let pcs_v = match (pcs_va, pcs_vb, pcs_vc) {
            (Some(a), Some(b), Some(c)) => Some((a + b + c) / 3.0),
            _ => None,
        };
        // 计算线电压
        let pcs_line_voltage = pcs_v.map(|v| v * 3_f64.sqrt());
        //PCS 总输出有功功率
        let pcs_power = center
            .read("pcs", 11)
            .and_then(|it| f64::try_from(it.value).ok());
        let pcs_ia = center
            .read("pcs", 4)
            .and_then(|it| f64::try_from(it.value).ok());
        let pcs_ib = center
            .read("pcs", 5)
            .and_then(|it| f64::try_from(it.value).ok());
        let pcs_ic = center
            .read("pcs", 6)
            .and_then(|it| f64::try_from(it.value).ok());
        let pcs_current = match (pcs_ia, pcs_ib, pcs_ic) {
            (Some(a), Some(b), Some(c)) => Some((a + b + c) / 3.0),
            _ => None,
        };
        //电网频率
        let pcs_frequency = center
            .read("pcs", 7)
            .and_then(|it| f64::try_from(it.value).ok());
        HomeAcData {
            voltage: pcs_line_voltage,
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
        //bcu 电池总压
        let voltage = center
            .read("bcu", 44)
            .and_then(|it| f64::try_from(it.value).ok());
        let highest_single_voltage = center
            .read("bcu", 9)
            .and_then(|it| f64::try_from(it.value).ok());
        let lowest_single_voltage = center
            .read("bcu", 13)
            .and_then(|it| f64::try_from(it.value).ok());
        let current = center
            .read("bcu", 46)
            .and_then(|it| f64::try_from(it.value).ok());
        let power = voltage
            .zip(current)
            .map(|(u, i)| ((u * i) / 1000.0 * 100.0).round() / 100.0);
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
            current,
            power,
            avg_temp,
            highest_temp,
            lowest_temp,
        }
    }
}

#[derive(Serialize)]
struct HomeEmuData {
    operation_mode: u8,
    permission: u8,
    health_status: u8,
}

impl HomeEmuData {
    fn new(center: &SharedPointCenter) -> Self {
        let operation_mode = center
            .read("emu", 1)
            .map(|it| it.value.as_u32().unwrap_or(0))
            .unwrap_or(0) as u8;
        let permission = center
            .read("emu", 2)
            .map(|it| it.value.as_u32().unwrap_or(3))
            .unwrap_or(3) as u8;
        let health_status = center
            .read("emu", 3)
            .map(|it| it.value.as_u32().unwrap_or(2))
            .unwrap_or(2) as u8;
        Self {
            operation_mode,
            permission,
            health_status,
        }
    }
}

#[derive(Serialize)]
struct HomeWarnData {
    pub id: PointId,
    pub key: &'static str,
    pub name: &'static str,
}

#[derive(Serialize)]
struct HomeWarnDatas(Vec<HomeWarnData>);

impl HomeWarnDatas {
    fn new(center: &SharedPointCenter) -> Self {
        let vec = center
            .read_range("emu", 500, 2000)
            .into_iter()
            .filter(|it| it.value == Val::U8(1))
            .map(|it| HomeWarnData {
                id: it.id,
                key: it.key,
                name: it.name,
            })
            .collect();
        HomeWarnDatas(vec)
    }
}

#[derive(Serialize)]
struct HomeCommonData {
    emu: HomeEmuData,
    ac: HomeAcData,
    dc: HomeDcData,
    warns: HomeWarnDatas,
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

async fn handle_home_ws(ws: &mut WebSocket, center: SharedPointCenter) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let emu_data = HomeEmuData::new(&center);
                let ac_data = HomeAcData::new(&center);
                let dc_data = HomeDcData::new(&center);
                let warn_datas = HomeWarnDatas::new(&center);
                let home_common_data = HomeCommonData {
                    emu: emu_data,
                    ac: ac_data,
                    dc: dc_data,
                    warns: warn_datas,
                };
                if let Ok(json) = serde_json::to_string(&home_common_data) {
                    let _ = ws.send(Message::text(json)).await;
                }
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

/// TDengine 按1分钟聚合查询返回的单行
#[derive(Debug, Deserialize)]
struct AggRow {
    ts: i64,
    val: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
struct HistoryPoint {
    ts: i64,
    value: Option<f64>,
}

#[derive(Debug, Serialize)]
struct HistoryCurves {
    //PCS总输出有功功率，1分钟聚合
    pcs_power: Vec<HistoryPoint>,
    //显示SOC，1分钟聚合
    soc: Vec<HistoryPoint>,
}

/// 拼出按1分钟聚合的查询语句：`_wstart` 转为毫秒时间戳别名为 ts，聚合值别名为 val
fn build_agg_sql(table: &str, column: &str, start: &str, end: &str) -> String {
    format!(
        "SELECT CAST(_wstart AS BIGINT) AS ts, AVG({column}) AS val FROM emu.{table} \
         WHERE ts >= '{start}' AND ts < '{end}' INTERVAL(1m)"
    )
}

async fn query_history() -> Result<HistoryCurves, TaosDbError> {
    let today = chrono::Local::now().date_naive();
    let start = today.and_hms_opt(0, 0, 0).unwrap();
    let end = start + chrono::Duration::days(1);
    let start = start.format("%Y-%m-%d %H:%M:%S").to_string();
    let end = end.format("%Y-%m-%d %H:%M:%S").to_string();

    let pcs_sql = build_agg_sql("pcs_data", "pcs_p_total", &start, &end);
    let soc_sql = build_agg_sql("bcu_data", "soc", &start, &end);

    let pcs_rows: Vec<AggRow> = query_rows(&pcs_sql).await?;
    let soc_rows: Vec<AggRow> = query_rows(&soc_sql).await?;

    Ok(HistoryCurves {
        pcs_power: pcs_rows
            .into_iter()
            .map(|r| HistoryPoint {
                ts: r.ts,
                value: r.val,
            })
            .collect(),
        soc: soc_rows
            .into_iter()
            .map(|r| HistoryPoint {
                ts: r.ts,
                value: r.val,
            })
            .collect(),
    })
}

#[handler]
pub async fn history_ws_handler(
    req: &mut Request,
    res: &mut Response,
    _depot: &mut Depot,
) -> Result<(), StatusError> {
    WebSocketUpgrade::new()
        .upgrade(req, res, |mut ws| async move {
            handle_history_ws(&mut ws).await;
        })
        .await
}

async fn handle_history_ws(ws: &mut WebSocket) {
    // 1分钟聚合窗口，无需更高频率刷新；tokio::time::interval 首次 tick 立即触发，
    // 相当于连接建立后立即推送一次当天曲线
    let mut ticker = tokio::time::interval(Duration::from_secs(60));
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match query_history().await {
                    Ok(data) => {
                        if let Ok(json) = serde_json::to_string(&data)
                            && ws.send(Message::text(json)).await.is_err() {
                                break;
                            }
                    }
                    Err(e) => {
                        tracing::error!("查询历史曲线数据失败: {}", e);
                    }
                }
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
