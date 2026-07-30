use chrono::{DateTime, Duration, Local, Months};
use collector_core::utils::taos::query_rows;
use serde::{Deserialize, Serialize};

use crate::services::{Service, ServiceError, ServiceResult};

/// pcs_data 可选查询字段白名单
const PCS_FIELDS: &[&str] = &[
    "pcs_p_total",
    "pcs_q_total",
    "pcs_s_total",
    "pcs_pf_total",
    "pcs_input_power",
    "pcs_ac_charge_energy",
    "pcs_ac_discharge_energy",
    "pcs_dc_charge_energy",
    "pcs_dc_discharge_energy",
];

/// bcu_data 可选查询字段白名单
const BCU_FIELDS: &[&str] = &[
    "cell_max_voltage",
    "cell_min_voltage",
    "cell_avg_voltage",
    "temp_max",
    "temp_min",
    "temp_avg",
    "soc",
    "chargeable_capacity",
    "dischargeable_capacity",
    "last_charge_capacity",
    "last_discharge_capacity",
    "total_charge_energy_high",
    "total_charge_energy_low",
    "total_discharge_energy_high",
    "total_discharge_energy_low",
    "total_voltage",
    "total_current",
];

/// 支持的聚合窗口白名单，直接拼进 TDengine `INTERVAL(...)` 子句
/// `1s` 数据量太大，只允许查询当天范围内的数据，见 `validate_today_range`
const ALLOWED_INTERVALS: &[&str] = &["1s", "1m", "5m", "15m", "30m", "1h", "6h", "1d"];

/// 最多允许查询3个自然月
const MAX_RANGE_MONTHS: u32 = 3;

#[derive(Debug, Clone, Deserialize)]
pub struct HistoryQueryParams {
    /// 查询起始时间，毫秒时间戳
    pub start: i64,
    /// 查询结束时间，毫秒时间戳
    pub end: i64,
    /// 逗号分隔的字段名，不传则查询全部可选字段
    pub fields: Option<String>,
    /// 聚合窗口，如 1m/5m/1h，不传默认 5m
    pub interval: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct HistoryPoint {
    pub ts: i64,
    pub value: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct FieldHistory {
    pub field: String,
    pub points: Vec<HistoryPoint>,
}

/// TDengine 按聚合窗口查询返回的单行
#[derive(Debug, Deserialize)]
struct AggRow {
    ts: i64,
    val: Option<f64>,
}

pub struct HistoryService {}

impl Service for HistoryService {}

impl HistoryService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {})
    }

    pub async fn pcs_history(
        &self,
        params: HistoryQueryParams,
    ) -> ServiceResult<Vec<FieldHistory>> {
        self.query(params, "pcs_data", PCS_FIELDS).await
    }

    pub async fn bcu_history(
        &self,
        params: HistoryQueryParams,
    ) -> ServiceResult<Vec<FieldHistory>> {
        self.query(params, "bcu_data", BCU_FIELDS).await
    }

    async fn query(
        &self,
        params: HistoryQueryParams,
        table: &str,
        allowed_fields: &[&str],
    ) -> ServiceResult<Vec<FieldHistory>> {
        let (start, end) = validate_range(params.start, params.end)?;
        let fields = resolve_fields(params.fields.as_deref(), allowed_fields)?;
        let interval = resolve_interval(params.interval.as_deref())?;
        if interval == "1s" {
            validate_today_range(start, end)?;
        }

        let start = start.format("%Y-%m-%d %H:%M:%S").to_string();
        let end = end.format("%Y-%m-%d %H:%M:%S").to_string();

        let mut result = Vec::with_capacity(fields.len());
        for field in fields {
            let sql = build_agg_sql(table, &field, &start, &end, interval);
            let rows: Vec<AggRow> = query_rows(&sql).await?;
            result.push(FieldHistory {
                field,
                points: rows
                    .into_iter()
                    .map(|r| HistoryPoint {
                        ts: r.ts,
                        value: r.val,
                    })
                    .collect(),
            });
        }
        Ok(result)
    }
}

/// 拼出按聚合窗口查询的语句：`_wstart` 转为毫秒时间戳别名为 ts，聚合值别名为 val
fn build_agg_sql(table: &str, field: &str, start: &str, end: &str, interval: &str) -> String {
    format!(
        "SELECT CAST(_wstart AS BIGINT) AS ts, ROUND(AVG({field}), 2) AS val FROM emu.{table} \
         WHERE ts >= '{start}' AND ts < '{end}' INTERVAL({interval})"
    )
}

fn validate_range(start: i64, end: i64) -> ServiceResult<(DateTime<Local>, DateTime<Local>)> {
    if start >= end {
        return Err(ServiceError::InvalidParameter(
            "start必须小于end".to_string(),
        ));
    }
    let start = DateTime::from_timestamp_millis(start)
        .ok_or_else(|| ServiceError::InvalidParameter("start时间戳不合法".to_string()))?
        .with_timezone(&Local);
    let end = DateTime::from_timestamp_millis(end)
        .ok_or_else(|| ServiceError::InvalidParameter("end时间戳不合法".to_string()))?
        .with_timezone(&Local);
    let max_end = start
        .checked_add_months(Months::new(MAX_RANGE_MONTHS))
        .ok_or_else(|| ServiceError::InvalidParameter("start时间戳不合法".to_string()))?;
    if end > max_end {
        return Err(ServiceError::InvalidParameter(format!(
            "查询时间范围最多{MAX_RANGE_MONTHS}个月"
        )));
    }
    Ok((start, end))
}

/// `1s` 聚合数据量太大，限制只能查询当天范围内的数据
fn validate_today_range(start: DateTime<Local>, end: DateTime<Local>) -> ServiceResult<()> {
    let now = Local::now();
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(Local)
        .unwrap();
    let today_end = today_start + Duration::days(1);
    if start < today_start || end > today_end {
        return Err(ServiceError::InvalidParameter(
            "聚合窗口为1s时，查询范围只能在当天内".to_string(),
        ));
    }
    Ok(())
}

fn resolve_fields(requested: Option<&str>, allowed: &[&str]) -> ServiceResult<Vec<String>> {
    let fields: Vec<String> = match requested {
        Some(s) if !s.trim().is_empty() => s
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
        _ => return Ok(allowed.iter().map(|s| s.to_string()).collect()),
    };
    for field in &fields {
        if !allowed.contains(&field.as_str()) {
            return Err(ServiceError::InvalidParameter(format!(
                "不支持的字段: {field}"
            )));
        }
    }
    Ok(fields)
}

fn resolve_interval(interval: Option<&str>) -> ServiceResult<&str> {
    let interval = interval.unwrap_or("5m");
    ALLOWED_INTERVALS
        .iter()
        .find(|&&i| i == interval)
        .copied()
        .ok_or_else(|| ServiceError::InvalidParameter(format!("不支持的聚合窗口: {interval}")))
}
