use std::time::Duration;

use collector_core::{
    center::data_center,
    core::point::{DataPoint, PointId, Val, Words},
    utils::{
        eg25::Eg25Info,
        taos::{TaosDbError, query_rows},
    },
};
use salvo::{
    Depot, Request, Response, handler,
    http::StatusError,
    prelude::WebSocketUpgrade,
    websocket::{Message, WebSocket},
};
use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
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
pub async fn data_ws_handler(req: &mut Request, res: &mut Response) -> Result<(), StatusError> {
    let query = req
        .parse_queries::<DevQueryParams>()
        .map_err(|_| StatusError::bad_request())?;

    WebSocketUpgrade::new()
        .upgrade(req, res, move |mut ws| async move {
            handle_ws(&mut ws, query).await;
        })
        .await
}

#[derive(Debug, Clone, Serialize)]
struct Point<'a> {
    id: u32,
    key: &'a str,
    name: &'a str,
    value: &'a Val,
    #[serde(skip_serializing_if = "Option::is_none")]
    words: Option<&'a Words>,
    #[serde(skip_serializing_if = "Option::is_none")]
    unit: Option<&'a str>,
}

impl<'a> Point<'a> {
    fn from_data_point(data_point: &'a DataPoint, lang: DevQueryLang) -> Self {
        let name = match lang {
            DevQueryLang::Zh => data_point.name.as_ref(),
            DevQueryLang::En => data_point
                .translator
                .as_ref()
                .map_or(data_point.name.as_ref(), |t| t.en.as_ref()),
        };
        Point {
            id: data_point.id,
            key: data_point.key.as_ref(),
            name,
            value: &data_point.value,
            words: data_point.words.as_deref(),
            unit: data_point.unit.as_deref(),
        }
    }
}

const PUSH_THROTTLE: Duration = Duration::from_millis(500);

#[cfg(test)]
mod metadata_tests {
    use super::*;
    use collector_core::core::point::Translator;
    use std::sync::Arc;

    #[test]
    fn shared_metadata_preserves_websocket_json() {
        let point = DataPoint {
            id: 1,
            key: "voltage".into(),
            name: "电压".into(),
            value: Val::U16(230),
            translator: Some(Arc::new(Translator {
                en: "Voltage".into(),
            })),
            bits: None,
            words: Some(Arc::new(Words::try_from("0 停止|Stop").unwrap())),
            unit: Some("V".into()),
            level: None,
        };
        for (lang, name) in [(DevQueryLang::Zh, "电压"), (DevQueryLang::En, "Voltage")] {
            let json = serde_json::to_value(Point::from_data_point(&point, lang)).unwrap();
            assert_eq!(
                json,
                serde_json::json!({
                    "id": 1, "key": "voltage", "name": name, "value": 230,
                    "words": {"0": {"zh": "停止", "en": "Stop"}}, "unit": "V"
                })
            );
        }
    }
}

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

async fn handle_ws(ws: &mut WebSocket, query: DevQueryParams) {
    let Some(mut rx) = data_center().subscribe(&query.dev) else {
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

/// 首页推送的数值统一保留 3 位小数（前端最多显示 3 位）：
/// 既避免 f64 运算噪声（如 380.12345678901234）撑大 JSON，也让"变化检测"能正确命中
fn ser_round3<S: serde::Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) => s.serialize_some(&((x * 1000.0).round() / 1000.0)),
        None => s.serialize_none(),
    }
}

#[derive(Debug, Serialize)]
struct HomeAcData {
    #[serde(serialize_with = "ser_round3")]
    voltage: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    current: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    power: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    frequency: Option<f64>,
}

impl HomeAcData {
    fn new() -> Self {
        let center = data_center();
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
    #[serde(serialize_with = "ser_round3")]
    soc: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    voltage: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    highest_single_voltage: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    lowest_single_voltage: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    current: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    power: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    avg_temp: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    highest_temp: Option<f64>,
    #[serde(serialize_with = "ser_round3")]
    lowest_temp: Option<f64>,
}

impl HomeDcData {
    fn new() -> Self {
        let center = data_center();
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
    /// 并离网状态：0非并网非离网 1并网 2离网 3状态冲突
    grid_mode: u8,
    /// EMU运行模式：0计划自动 1总功率
    run_mode: u8,
    /// EMU控制源：0本地 1远程
    control_source: u8,
}

impl HomeEmuData {
    fn new() -> Self {
        let center = data_center();
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
        let grid_mode = center
            .read("emu", 17)
            .map(|it| it.value.as_u32().unwrap_or(0))
            .unwrap_or(0) as u8;
        let run_mode = center
            .read("emu", 8)
            .map(|it| it.value.as_u32().unwrap_or(1))
            .unwrap_or(1) as u8;
        let control_source = center
            .read("emu", 9)
            .map(|it| it.value.as_u32().unwrap_or(0))
            .unwrap_or(0) as u8;
        Self {
            operation_mode,
            permission,
            health_status,
            grid_mode,
            run_mode,
            control_source,
        }
    }
}

#[derive(Serialize)]
struct HomeWarnData {
    pub id: PointId,
    pub key: std::sync::Arc<str>,
    pub name: std::sync::Arc<str>,
}

#[derive(Serialize)]
struct HomeWarnDatas(Vec<HomeWarnData>);

impl HomeWarnDatas {
    fn new() -> Self {
        let center = data_center();
        let vec = center
            .read_range("emu", 500, 2000)
            .into_iter()
            .filter(|it| it.value == Val::U8(1))
            .map(|it| HomeWarnData {
                id: it.id,
                key: it.key.clone(),
                name: it.name.clone(),
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
pub async fn home_ws_handler(req: &mut Request, res: &mut Response) -> Result<(), StatusError> {
    WebSocketUpgrade::new()
        .upgrade(req, res, |mut ws| async move {
            handle_home_ws(&mut ws).await;
        })
        .await
}

async fn handle_home_ws(ws: &mut WebSocket) {
    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    // 上一次推送的 JSON；数据没变化就不推送（4G 流量有限），前端保持上次的值
    let mut last_json = String::new();
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                let emu_data = HomeEmuData::new();
                let ac_data = HomeAcData::new();
                let dc_data = HomeDcData::new();
                let warn_datas = HomeWarnDatas::new();
                let home_common_data = HomeCommonData {
                    emu: emu_data,
                    ac: ac_data,
                    dc: dc_data,
                    warns: warn_datas,
                };
                if let Ok(json) = serde_json::to_string(&home_common_data)
                    && json != last_json
                {
                    if ws.send(Message::text(json.clone())).await.is_err() {
                        break;
                    }
                    last_json = json;
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

/// 聚合窗口（INTERVAL(1m)），列式序列按该步长等间隔排布
const HISTORY_STEP_MS: i64 = 60_000;

/// 紧凑曲线：第 i 个值对应时间 `start + i * step`，缺失的窗口填 null。
/// 相比逐点 `{"ts":..,"value":..}`，省掉了重复的字段名和每点 13 位时间戳，
/// 且数值按显示精度取整，体积约为原来的 1/7
#[derive(Debug, Serialize)]
struct CompactSeries {
    /// 第一个点的时间戳(ms)；无数据时为 0
    start: i64,
    values: Vec<Option<f64>>,
}

impl CompactSeries {
    /// `points` 须按 ts 升序；`decimals` 为保留的小数位数（与前端显示精度一致）
    fn from_points(points: &[HistoryPoint], decimals: i32) -> Self {
        let Some(first) = points.first() else {
            return Self {
                start: 0,
                values: Vec::new(),
            };
        };
        let start = first.ts;
        let k = 10f64.powi(decimals);
        let mut values: Vec<Option<f64>> = Vec::with_capacity(points.len());
        for p in points {
            let idx = ((p.ts - start) / HISTORY_STEP_MS).max(0) as usize;
            if idx < values.len() {
                continue; // 同一窗口重复，保留先到的
            }
            values.resize(idx, None);
            values.push(p.value.map(|v| (v * k).round() / k));
        }
        Self { start, values }
    }
}

#[derive(Debug, Serialize)]
struct HistoryCurves {
    /// true：全量（当天 0 点至今），前端整体替换；
    /// false：增量，前端按 ts 合并（同 ts 覆盖，新 ts 追加）
    full: bool,
    /// 列式序列的步长(ms)，前端据此还原时间戳
    step: i64,
    //PCS总输出有功功率，1分钟聚合，保留 2 位小数
    pcs_power: CompactSeries,
    //显示SOC，1分钟聚合，保留 1 位小数
    soc: CompactSeries,
}

/// 单个连接已推送的进度：基准日期 + 两条曲线各自最后一个点的时间戳(ms)
struct HistoryCursor {
    day: chrono::NaiveDate,
    pcs_last: Option<i64>,
    soc_last: Option<i64>,
}

/// 拼出按1分钟聚合的查询语句：`_wstart` 转为毫秒时间戳别名为 ts，聚合值别名为 val
fn build_agg_sql(table: &str, column: &str, start: &str, end: &str) -> String {
    format!(
        "SELECT CAST(_wstart AS BIGINT) AS ts, AVG({column}) AS val FROM emu.{table} \
         WHERE ts >= '{start}' AND ts < '{end}' INTERVAL(1m)"
    )
}

const TS_FMT: &str = "%Y-%m-%d %H:%M:%S";

/// 查询一条曲线。`since_ms` 为 None 时从当天 0 点查全量，否则从该时间戳所在分钟起查增量（含）
async fn query_series(
    table: &str,
    column: &str,
    day_start: &str,
    day_end: &str,
    since_ms: Option<i64>,
) -> Result<Vec<HistoryPoint>, TaosDbError> {
    let start = since_ms
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
        .map(|dt| dt.format(TS_FMT).to_string())
        .unwrap_or_else(|| day_start.to_string());
    let rows: Vec<AggRow> = query_rows(&build_agg_sql(table, column, &start, day_end)).await?;
    Ok(rows
        .into_iter()
        .map(|r| HistoryPoint {
            ts: r.ts,
            value: r.val,
        })
        .collect())
}

/// 按游标查询：游标为空或已跨天则返回全量并重置游标，否则只返回增量。
/// 任一查询失败时游标保持不变，下个周期会补发。
async fn query_history(cursor: &mut Option<HistoryCursor>) -> Result<HistoryCurves, TaosDbError> {
    let today = chrono::Local::now().date_naive();
    let day_start = today.and_hms_opt(0, 0, 0).unwrap();
    let day_end = (day_start + chrono::Duration::days(1))
        .format(TS_FMT)
        .to_string();
    let day_start = day_start.format(TS_FMT).to_string();

    let (full, pcs_since, soc_since) = match cursor.as_ref() {
        Some(c) if c.day == today => (false, c.pcs_last, c.soc_last),
        _ => (true, None, None),
    };

    let pcs_power = query_series("pcs_data", "pcs_p_total", &day_start, &day_end, pcs_since).await?;
    let soc = query_series("bcu_data", "soc", &day_start, &day_end, soc_since).await?;

    // 推进游标：取本次最后一个点的 ts；增量为空时沿用旧值
    let pcs_last = pcs_power.last().map(|p| p.ts).or(pcs_since);
    let soc_last = soc.last().map(|p| p.ts).or(soc_since);
    *cursor = Some(HistoryCursor {
        day: today,
        pcs_last,
        soc_last,
    });

    Ok(HistoryCurves {
        full,
        step: HISTORY_STEP_MS,
        pcs_power: CompactSeries::from_points(&pcs_power, 2),
        soc: CompactSeries::from_points(&soc, 1),
    })
}

#[handler]
pub async fn eg25_ws_handler(
    req: &mut Request,
    res: &mut Response,
    depot: &mut Depot,
) -> Result<(), StatusError> {
    let rx = depot
        .get::<watch::Receiver<Eg25Info>>("eg25")
        .map_err(|_| StatusError::service_unavailable())?
        .clone();

    WebSocketUpgrade::new()
        .upgrade(req, res, move |mut ws| async move {
            handle_eg25_ws(&mut ws, rx).await;
        })
        .await
}

async fn handle_eg25_ws(ws: &mut WebSocket, mut rx: watch::Receiver<Eg25Info>) {
    // 建立连接后立即推送当前状态
    let initial = rx.borrow().clone();
    if let Ok(json) = serde_json::to_string(&initial)
        && ws.send(Message::text(json)).await.is_err()
    {
        return;
    }

    loop {
        tokio::select! {
            result = rx.changed() => {
                if result.is_err() { break; }
                let data = rx.borrow().clone();
                if let Ok(json) = serde_json::to_string(&data)
                    && ws.send(Message::text(json)).await.is_err() {
                        break;
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
    // 首包全量，之后只推增量（4G 流量有限）；跨天时自动再发一次全量
    let mut cursor: Option<HistoryCursor> = None;
    loop {
        tokio::select! {
            _ = ticker.tick() => {
                match query_history(&mut cursor).await {
                    Ok(data) => {
                        // 增量为空则不推送
                        if !data.full && data.pcs_power.values.is_empty() && data.soc.values.is_empty() {
                            continue;
                        }
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

#[cfg(test)]
mod history_compact_tests {
    use super::*;

    fn pt(min: i64, v: f64) -> HistoryPoint {
        HistoryPoint {
            ts: 1_764_000_000_000 + min * HISTORY_STEP_MS,
            value: Some(v),
        }
    }

    #[test]
    fn 空洞填null且取整() {
        let s = CompactSeries::from_points(&[pt(0, 1.23456), pt(1, 2.0), pt(4, 3.987)], 2);
        assert_eq!(s.start, 1_764_000_000_000);
        assert_eq!(
            s.values,
            vec![Some(1.23), Some(2.0), None, None, Some(3.99)]
        );
    }

    #[test]
    fn 无数据() {
        let s = CompactSeries::from_points(&[], 2);
        assert!(s.values.is_empty());
    }

    #[test]
    fn 体积对比() {
        let pts: Vec<_> = (0..950).map(|i| pt(i, 123.456789012345 + i as f64)).collect();
        let old = serde_json::to_string(&pts).unwrap().len();
        let new = serde_json::to_string(&CompactSeries::from_points(&pts, 2))
            .unwrap()
            .len();
        println!("旧 {old} 字节 -> 新 {new} 字节");
        assert!(new * 4 < old);
    }
}
