use collector_core::utils::database::get_database;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::{
    dao::electricity::ElectricityDao,
    handlers::electricity::{PeriodParams, UpdatePricesParams},
    models::electricity::{ElectricityPeriod, PeriodType},
    services::{ServiceError, ServiceResult},
};

pub(crate) const MINUTES_PER_DAY: u32 = 24 * 60;
/// 计划曲线的时间粒度，时段边界必须与之对齐，套利曲线才能按时段准确取价
const SLOT_MINUTES: u32 = 15;

#[derive(Debug, Serialize)]
pub struct PriceView {
    pub period_type: PeriodType,
    pub name: &'static str,
    /// 单价(元/kWh)，尚未配置为 null
    pub price: Option<f64>,
}

/// 解析 `HH:MM` 为当天的分钟数，仅 `24:00` 可取到一天末尾
pub(crate) fn parse_minutes(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    if h.len() != 2 || m.len() != 2 {
        return None;
    }
    let (h, m) = (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?);
    let total = h * 60 + m;
    (m < 60 && total <= MINUTES_PER_DAY).then_some(total)
}

fn format_minutes(total: u32) -> String {
    format!("{:02}:{:02}", total / 60, total % 60)
}

pub struct ElectricityService {
    pool: SqlitePool,
}

impl ElectricityService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            pool: get_database()?,
        })
    }

    /// 固定返回尖峰平谷四项，未配置的单价为 null
    pub async fn prices(&self) -> ServiceResult<Vec<PriceView>> {
        let saved = ElectricityDao::find_prices(&self.pool).await?;
        Ok(PeriodType::ALL
            .into_iter()
            .map(|period_type| PriceView {
                period_type,
                name: period_type.name(),
                price: saved
                    .iter()
                    .find(|p| p.period_type == period_type)
                    .map(|p| p.price),
            })
            .collect())
    }

    /// 部分更新：只写入传了的类型
    pub async fn update_prices(
        &self,
        params: UpdatePricesParams,
        updated_by: Option<String>,
    ) -> ServiceResult<()> {
        let prices: Vec<(PeriodType, f64)> = [
            (PeriodType::Sharp, params.sharp),
            (PeriodType::Peak, params.peak),
            (PeriodType::Flat, params.flat),
            (PeriodType::Valley, params.valley),
        ]
        .into_iter()
        .filter_map(|(t, p)| p.map(|p| (t, p)))
        .collect();
        if prices.is_empty() {
            return Err(ServiceError::InvalidParameter(
                "没有需要更新的电价".to_string(),
            ));
        }
        ElectricityDao::upsert_prices(&self.pool, &prices, updated_by.as_deref()).await?;
        Ok(())
    }

    pub async fn periods(&self) -> ServiceResult<Vec<ElectricityPeriod>> {
        Ok(ElectricityDao::find_periods(&self.pool).await?)
    }

    /// 整体替换时段表。时段按开始时间排序后必须从 00:00 起首尾相接、直到 24:00，
    /// 不留空档也不重叠，这样一天内任意时刻都能对应到唯一的电价类型。
    pub async fn replace_periods(
        &self,
        params: Vec<PeriodParams>,
        updated_by: Option<String>,
    ) -> ServiceResult<()> {
        if params.is_empty() {
            return Err(ServiceError::InvalidParameter("时段不能为空".to_string()));
        }
        let mut parsed = Vec::with_capacity(params.len());
        for p in &params {
            let start = parse_minutes(&p.start_time)
                .filter(|m| *m < MINUTES_PER_DAY)
                .ok_or_else(|| {
                    ServiceError::InvalidParameter(format!(
                        "开始时间`{}`格式错误，应为 00:00-23:59 的 HH:MM",
                        p.start_time
                    ))
                })?;
            let end = parse_minutes(&p.end_time).ok_or_else(|| {
                ServiceError::InvalidParameter(format!(
                    "结束时间`{}`格式错误，应为 HH:MM，最大 24:00",
                    p.end_time
                ))
            })?;
            if end <= start {
                return Err(ServiceError::InvalidParameter(format!(
                    "时段{}-{}结束时间必须晚于开始时间，跨零点的时段请拆成两段",
                    p.start_time, p.end_time
                )));
            }
            if start % SLOT_MINUTES != 0 || end % SLOT_MINUTES != 0 {
                return Err(ServiceError::InvalidParameter(format!(
                    "时段{}-{}的边界必须是{}分钟的整数倍",
                    p.start_time, p.end_time, SLOT_MINUTES
                )));
            }
            parsed.push((start, end, p.period_type));
        }
        parsed.sort_by_key(|(start, _, _)| *start);

        let mut cursor = 0;
        for (start, end, _) in &parsed {
            if *start < cursor {
                return Err(ServiceError::InvalidParameter(format!(
                    "时段存在重叠，重叠位置在{}",
                    format_minutes(*start)
                )));
            }
            if *start > cursor {
                return Err(ServiceError::InvalidParameter(format!(
                    "时段存在空档：{}-{}未配置",
                    format_minutes(cursor),
                    format_minutes(*start)
                )));
            }
            cursor = *end;
        }
        if cursor != MINUTES_PER_DAY {
            return Err(ServiceError::InvalidParameter(format!(
                "时段未覆盖全天：{}-24:00未配置",
                format_minutes(cursor)
            )));
        }

        let periods: Vec<ElectricityPeriod> = parsed
            .into_iter()
            .map(|(start, end, period_type)| ElectricityPeriod {
                start_time: format_minutes(start),
                end_time: format_minutes(end),
                period_type,
            })
            .collect();
        ElectricityDao::replace_periods(&self.pool, &periods, updated_by.as_deref()).await?;
        Ok(())
    }
}
