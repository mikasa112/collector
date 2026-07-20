use std::collections::HashSet;

use chrono::NaiveDate;
use collector_core::utils::database::get_database;
use serde::Serialize;
use sqlx::SqlitePool;

use crate::{
    dao::planned_curve::{PlanCurveDetailDao, PlanCurveMasterDao},
    handlers::planned_curve::{BindPlannedCurveDetailsParams, CreatePlannedCurveParams},
    models::planned_curve::{CurveType, PlanCurveDetail, PlanCurveMaster},
    services::{ServiceError, ServiceResult},
};

#[derive(Debug, Serialize)]
pub struct PlanCurveMasterSimpleResp {
    pub id: u32,
    pub curve_name: String,
    pub curve_type: CurveType,
    //优先级，数字越小优先级越高
    pub priority: Option<u8>,
    //状态：0-草稿 1-已发布 2-执行中 3-已归档
    pub status: Option<u8>,
    //生效起始时间
    pub valid_start_date: Option<String>,
    //生效结束时间
    pub valid_end_date: Option<String>,
    //生效星期掩码，如 "1,2,3,4,5" 表示周一至周五
    pub effective_weekdays: Option<String>,
    pub created_by: Option<String>,
    pub remark: Option<String>,
}

impl From<PlanCurveMaster> for PlanCurveMasterSimpleResp {
    fn from(value: PlanCurveMaster) -> Self {
        PlanCurveMasterSimpleResp {
            id: value.id,
            curve_name: value.curve_name,
            curve_type: value.curve_type,
            priority: value.priority,
            status: value.status,
            valid_start_date: value.valid_start_date,
            valid_end_date: value.valid_end_date,
            effective_weekdays: value.effective_weekdays,
            created_by: value.created_by,
            remark: value.remark,
        }
    }
}

pub struct PlannedCurveService {
    pool: SqlitePool,
}

impl PlannedCurveService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            pool: get_database()?,
        })
    }

    pub async fn planned_curve_list(
        &self,
        page: u32,
        size: u32,
    ) -> ServiceResult<(Vec<PlanCurveMasterSimpleResp>, usize)> {
        let result = PlanCurveMasterDao::find_all(&self.pool, size, (page - 1) * size).await?;
        let total = PlanCurveMasterDao::find_all_len(&self.pool).await?;
        let result = result
            .into_iter()
            .map(PlanCurveMasterSimpleResp::from)
            .collect::<Vec<_>>();
        Ok((result, total))
    }

    pub async fn find_master_by_id(&self, id: u32) -> ServiceResult<PlanCurveMasterSimpleResp> {
        let master = PlanCurveMasterDao::find_by_id(&self.pool, id).await?;
        if let Some(m) = master {
            Ok(PlanCurveMasterSimpleResp::from(m))
        } else {
            Err(ServiceError::NotFound(format!("{id}不存在").to_string()))
        }
    }

    pub async fn create_planned_curve_master(
        &self,
        params: CreatePlannedCurveParams,
    ) -> ServiceResult<()> {
        let valid_start_date = params.valid_start_date.as_deref();
        let valid_end_date = params.valid_end_date.as_deref();
        let start_date = validate_date(valid_start_date)?;
        let end_date = validate_date(valid_end_date)?;
        if start_date.is_some() && end_date.is_some() && end_date.unwrap() < start_date.unwrap() {
            return Err(ServiceError::InvalidParameter(String::from(
                "结束时间须在开始时间之后",
            )));
        };
        let _ = PlanCurveMasterDao::create(
            &self.pool,
            &params.curve_name,
            params
                .curve_type
                .ok_or_else(|| ServiceError::InvalidParameter("curve_type不能为空".to_string()))?,
            Some(params.priority.unwrap_or(5)),
            Some(params.status.unwrap_or(0)),
            valid_start_date,
            valid_end_date,
            params.effective_weekdays.as_deref(),
            params.created_by.as_deref(),
            params.remark.as_deref(),
        )
        .await?;
        Ok(())
    }

    pub async fn bind_planned_curve_details(
        &self,
        params: BindPlannedCurveDetailsParams,
    ) -> ServiceResult<()> {
        let current = PlanCurveMasterDao::find_by_id(&self.pool, params.curve_id).await?;
        if current.is_none() {
            return Err(ServiceError::NotFound(format!(
                "{}的计划曲线不存在",
                params.curve_id
            )));
        }
        if params.times.is_empty() {
            return Err(ServiceError::InvalidParameter(String::from(
                "时间段不能为空!",
            )));
        }
        let mut seen = HashSet::with_capacity(params.times.len());
        let mut times = Vec::with_capacity(params.times.len());
        for t in params.times.iter() {
            if t.time_index > 95 {
                return Err(ServiceError::InvalidParameter(format!(
                    "时间段索引 {} 超出范围(0-95)",
                    t.time_index
                )));
            }
            if !seen.insert(t.time_index) {
                return Err(ServiceError::InvalidParameter(format!(
                    "时间段索引 {} 重复",
                    t.time_index
                )));
            }
            times.push((t.time_index, t.power_value, t.soc_limit));
        }
        PlanCurveDetailDao::upsert_details(&self.pool, params.curve_id, times.as_slice()).await?;
        Ok(())
    }

    pub async fn planned_curve_details(
        &self,
        curve_id: u32,
    ) -> ServiceResult<Vec<PlanCurveDetail>> {
        let current = PlanCurveMasterDao::find_by_id(&self.pool, curve_id).await?;
        if current.is_none() {
            return Err(ServiceError::NotFound(format!(
                "{}的计划曲线不存在",
                curve_id
            )));
        }
        let list = PlanCurveDetailDao::query_by_master_id(&self.pool, curve_id).await?;
        Ok(list)
    }
}

fn validate_date(date_str: Option<&str>) -> ServiceResult<Option<NaiveDate>> {
    if let Some(date_str) = date_str {
        if let Ok(naive_date) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            return Ok(Some(naive_date));
        };
    } else {
        return Ok(None);
    }
    Err(ServiceError::InvalidParameter(format!(
        "{:?}时间不合法!",
        date_str
    )))
}
