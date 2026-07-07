use collector_core::utils::database::get_database;
use sqlx::SqlitePool;

use crate::{
    dao::planned_curve::PlanCurveMasterDao,
    models::planned_curve::PlanCurveMaster,
    services::{ServiceError, ServiceResult},
};

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
    ) -> ServiceResult<(Vec<PlanCurveMaster>, usize)> {
        let result = PlanCurveMasterDao::find_all(&self.pool, size, (page - 1) * size).await?;
        let total = PlanCurveMasterDao::find_all_len(&self.pool).await?;
        Ok((result, total))
    }

    pub async fn find_master_by_id(&self, id: u32) -> ServiceResult<PlanCurveMaster> {
        let master = PlanCurveMasterDao::find_by_id(&self.pool, id).await?;
        if let Some(m) = master {
            Ok(m)
        } else {
            return Err(ServiceError::NotFound(format!("{id}不存在").to_string()));
        }
    }
}
