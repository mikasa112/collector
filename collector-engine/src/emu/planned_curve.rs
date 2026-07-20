use std::time::Duration;

use chrono::{Datelike, Timelike};
use collector_core::{
    center::{DataCenterError, SharedPointCenter},
    core::point::{DownDataPoint, Val},
    down,
};
use sqlx::{SqlitePool, prelude::FromRow};

use crate::{
    DataDriven,
    strategy::{Schedule, Strategy, StrategyError},
};

#[derive(FromRow)]
struct PlanCurveMaster {
    id: u32,
    curve_name: String,
    //生效起始时间
    valid_start_date: Option<String>,
    //生效结束时间
    valid_end_date: Option<String>,
    //生效星期掩码，如 "1,2,3,4,5" 表示周一至周五
    effective_weekdays: Option<String>,
}

#[derive(FromRow)]
struct PlanCurveDetail {
    // 0-95，对应00:00-23:45, 时间粒度15分钟
    time_index: u8,
    //功率
    power_value: f64,
    //SOC限制
    soc_limit: Option<f64>,
}

#[derive(thiserror::Error, Debug)]
enum PlannedCurveError {
    #[error("数据库错误: {0}")]
    SQLError(#[from] sqlx::Error),
}

pub struct PlannedCurve {
    center: SharedPointCenter,
    pool: SqlitePool,
    //最近一次下发的 (curve_id, time_index)，避免同一时段重复下发
    last: Option<(u32, u8)>,
}

impl PlannedCurve {
    pub fn new(center: SharedPointCenter, pool: SqlitePool) -> Self {
        Self {
            center,
            pool,
            last: None,
        }
    }

    async fn active(&self) -> Result<Option<PlanCurveMaster>, PlannedCurveError> {
        let current_active = sqlx::query_as::<_, PlanCurveMaster>(
            "SELECT id, curve_name, valid_start_date, valid_end_date, effective_weekdays
            FROM t_plan_curve_master tpcm
            WHERE tpcm.status = 1
              AND tpcm.deleted_at is NULL
            ORDER BY tpcm.priority, tpcm.created_at",
        )
        .fetch_all(&self.pool)
        .await?;
        //生效日期/星期以本地日历日为准，而非 UTC，避免临近0点时的偏移
        let today = chrono::Local::now().date_naive();
        let weekday = today.weekday().number_from_monday() as u8;
        let plan = current_active.into_iter().find(|it| {
            let start_ok = it
                .valid_start_date
                .as_deref()
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .is_none_or(|d| d <= today);
            let end_ok = it
                .valid_end_date
                .as_deref()
                .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
                .is_none_or(|d| d >= today);
            let weekday_ok = it.effective_weekdays.as_deref().is_none_or(|mask| {
                mask.split(',')
                    .filter_map(|s| s.trim().parse::<u8>().ok())
                    .any(|d| d == weekday)
            });
            start_ok && end_ok && weekday_ok
        });
        Ok(plan)
    }

    /// 查询计划曲线详情（含功率为0的时段，用于主动清零）
    async fn active_details(&self, id: u32) -> Result<Vec<PlanCurveDetail>, PlannedCurveError> {
        let details = sqlx::query_as::<_, PlanCurveDetail>(
            "SELECT time_index,
                    CAST(power_value AS REAL) AS power_value,
                    CAST(soc_limit AS REAL) AS soc_limit
            FROM t_plan_curve_detail tpcd
            WHERE tpcd.curve_id = ?
              AND tpcd.deleted_at is NULL
            ORDER BY tpcd.time_index",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        Ok(details)
    }

    /// 根据当前生效曲线与时间段，下发对应的有功功率设定
    async fn apply(&mut self) {
        let Ok(active) = self.active().await else {
            tracing::debug!("[计划曲线] 查询生效曲线失败");
            return;
        };
        let Some(active) = active else {
            tracing::debug!("[计划曲线] 暂无生效曲线");
            return;
        };
        let Ok(details) = self.active_details(active.id).await else {
            tracing::debug!("[计划曲线] 查询曲线明细失败");
            return;
        };
        let now = chrono::Local::now();
        let time_index = (now.hour() * 4 + now.minute() / 15) as u8;
        let Some(detail) = details.iter().find(|d| d.time_index == time_index) else {
            return;
        };
        let key = (active.id, time_index);
        if self.last == Some(key) {
            return;
        }
        if let Some(limit) = detail.soc_limit
            && let Some(current_soc) = self.center.read("bcu", 32) {
                let soc = f64::try_from(current_soc.value);
                if let Ok(soc) = soc {
                    //正充负放：充电时SOC达到上限、放电时SOC达到下限，均改为下发功率0
                    let reach_limit = if detail.power_value > 0.0 {
                        soc >= limit
                    } else if detail.power_value < 0.0 {
                        soc <= limit
                    } else {
                        false
                    };
                    if reach_limit {
                        if let Err(e) = self
                            .center
                            .dispatch("pcs", vec![down!(id: 2003, Val::F64(0.0))])
                            .await
                        {
                            tracing::error!("[计划曲线] 下发功率失败: {}", e);
                            return;
                        }
                        tracing::info!(
                            "[计划曲线] 曲线「{}」第{}段 当前SOC {}% 达到限制 {}%，下发功率0kW",
                            active.curve_name,
                            time_index,
                            soc,
                            limit
                        );
                        return;
                    }
                }
            }
        if let Err(e) = self
            .center
            .dispatch("pcs", vec![down!(id: 2003, Val::F64(detail.power_value))])
            .await
        {
            tracing::error!("[计划曲线] 下发功率失败: {}", e);
            return;
        }
        tracing::info!(
            "[计划曲线] 曲线「{}」第{}段 下发有功功率 {}kW",
            active.curve_name,
            time_index,
            detail.power_value
        );
        self.last = Some(key);
    }
}

#[async_trait::async_trait]
impl Strategy for PlannedCurve {
    fn name(&self) -> &str {
        "计划曲线"
    }

    fn schedule(&self) -> Schedule {
        Schedule::Interval(Duration::from_mins(1))
    }

    async fn on_start(&mut self) -> Result<(), StrategyError> {
        self.apply().await;
        Ok(())
    }

    async fn on_tick(&mut self) -> Result<(), StrategyError> {
        self.apply().await;
        Ok(())
    }
}

#[async_trait::async_trait]
impl DataDriven for PlannedCurve {
    async fn down(&self, _points: &[DownDataPoint]) -> Result<(), DataCenterError> {
        Ok(())
    }
}
