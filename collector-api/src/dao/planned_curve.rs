use sqlx::SqlitePool;

use crate::{
    dao::error::DaoResult,
    models::planned_curve::{CurveType, PlanCurveDetail, PlanCurveMaster},
};

/// 计划曲线数据访问对象
pub struct PlanCurveMasterDao;

pub struct PlanCurveDetailDao;

/// 新建计划曲线主表记录所需参数
pub struct NewPlanCurveMaster<'a> {
    pub curve_name: &'a str,
    pub curve_type: CurveType,
    pub priority: Option<u8>,
    pub status: Option<u8>,
    pub valid_start_date: Option<&'a str>,
    pub valid_end_date: Option<&'a str>,
    pub effective_weekdays: Option<&'a str>,
    pub created_by: Option<&'a str>,
    pub remark: Option<&'a str>,
}

impl PlanCurveMasterDao {
    pub async fn create(pool: &SqlitePool, params: NewPlanCurveMaster<'_>) -> DaoResult<i64> {
        let result = sqlx::query(
            "INSERT INTO t_plan_curve_master (
                curve_name, curve_type, priority, status,
                valid_start_date, valid_end_date, effective_weekdays,
                created_by, remark
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(params.curve_name)
        .bind(params.curve_type)
        .bind(params.priority)
        .bind(params.status)
        .bind(params.valid_start_date)
        .bind(params.valid_end_date)
        .bind(params.effective_weekdays)
        .bind(params.created_by)
        .bind(params.remark)
        .execute(pool)
        .await?;
        Ok(result.last_insert_rowid())
    }

    pub async fn find_all_len(pool: &SqlitePool) -> DaoResult<usize> {
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)
            AS total
            FROM t_plan_curve_master tpcm
            WHERE tpcm.deleted_at is NULL",
        )
        .fetch_one(pool)
        .await?;
        Ok(total as usize)
    }

    pub async fn find_all(
        pool: &SqlitePool,
        limit: u32,
        offset: u32,
    ) -> DaoResult<Vec<PlanCurveMaster>> {
        let plan_curve_master = sqlx::query_as::<_, PlanCurveMaster>(
            "SELECT * FROM t_plan_curve_master tpcm
            WHERE deleted_at is NULL
            ORDER BY tpcm.updated_at DESC
            LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(pool)
        .await?;
        Ok(plan_curve_master)
    }

    #[allow(dead_code)]
    pub async fn find_active(pool: &SqlitePool) -> DaoResult<Vec<PlanCurveMaster>> {
        let current_active = sqlx::query_as::<_, PlanCurveMaster>(
            "SELECT * FROM t_plan_curve_master tpcm
            WHERE tpcm.status = 1
              AND tpcm.deleted_at is NULL
              AND tpcm.valid_start_date <= date('now')
              AND tpcm.valid_end_date >= date('now')
            ORDER BY tpcm.priority, tpcm.created_at",
        )
        .fetch_all(pool)
        .await?;
        Ok(current_active)
    }

    #[allow(dead_code)]
    pub async fn find_like_name(
        pool: &SqlitePool,
        like_name: &str,
    ) -> DaoResult<Vec<PlanCurveMaster>> {
        let plans = sqlx::query_as::<_, PlanCurveMaster>(
            "SELECT * FROM plan_curve_master
            WHERE curve_name LIKE '%?%'
              AND tpcm.status != 3
              AND tpcm.deleted_at is NULL;",
        )
        .bind(like_name)
        .fetch_all(pool)
        .await?;
        Ok(plans)
    }

    pub async fn find_by_id(pool: &SqlitePool, id: u32) -> DaoResult<Option<PlanCurveMaster>> {
        let plan = sqlx::query_as::<_, PlanCurveMaster>(
            "SELECT * FROM t_plan_curve_master WHERE id = ? AND deleted_at is NULL",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;
        Ok(plan)
    }
}

impl PlanCurveDetailDao {
    #[allow(dead_code)]
    pub async fn create(
        pool: &SqlitePool,
        curve_id: u32,
        time_index: u8,
        power_value: f64,
        soc_limit: Option<f64>,
    ) -> DaoResult<i64> {
        let result = sqlx::query(
            "INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit)
             VALUES (?, ?, ?, ?)",
        )
        .bind(curve_id)
        .bind(time_index)
        .bind(power_value)
        .bind(soc_limit)
        .execute(pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// 批量插入曲线明细，如一次性写入某条曲线的 96 个功率点
    #[allow(dead_code)]
    pub async fn batch_create(
        pool: &SqlitePool,
        curve_id: u32,
        details: &[(u8, f64, Option<f64>)],
    ) -> DaoResult<u64> {
        if details.is_empty() {
            return Ok(0);
        }

        let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit) ",
        );

        query_builder.push_values(details, |mut b, (time_index, power_value, soc_limit)| {
            b.push_bind(curve_id)
                .push_bind(*time_index)
                .push_bind(*power_value)
                .push_bind(*soc_limit);
        });
        let result = query_builder.build().execute(pool).await?;
        Ok(result.rows_affected())
    }

    /// 增量合并某条曲线的明细：已存在的 time_index 更新功率/SOC，不存在的插入，其余时间点不受影响
    pub async fn upsert_details(
        pool: &SqlitePool,
        curve_id: u32,
        details: &[(u8, f64, Option<f64>)],
    ) -> DaoResult<u64> {
        if details.is_empty() {
            return Ok(0);
        }

        let mut query_builder = sqlx::QueryBuilder::new(
            "INSERT INTO t_plan_curve_detail (curve_id, time_index, power_value, soc_limit) ",
        );
        query_builder.push_values(details, |mut b, (time_index, power_value, soc_limit)| {
            b.push_bind(curve_id)
                .push_bind(*time_index)
                .push_bind(*power_value)
                .push_bind(*soc_limit);
        });
        query_builder.push(
            " ON CONFLICT (curve_id, time_index) DO UPDATE SET
                power_value = excluded.power_value,
                soc_limit = excluded.soc_limit,
                updated_at = datetime('now', 'localtime')",
        );

        let result = query_builder.build().execute(pool).await?;
        Ok(result.rows_affected())
    }

    pub async fn query_by_master_id(pool: &SqlitePool, id: u32) -> DaoResult<Vec<PlanCurveDetail>> {
        let details = sqlx::query_as::<_, PlanCurveDetail>(
            "SELECT id, curve_id, time_index,
                    CAST(power_value AS REAL) AS power_value,
                    CAST(soc_limit AS REAL) AS soc_limit,
                    created_at, updated_at, deleted_at
            FROM t_plan_curve_detail tpcd
            WHERE tpcd.curve_id = ?
              AND tpcd.deleted_at is NULL
            ORDER BY tpcd.time_index",
        )
        .bind(id)
        .fetch_all(pool)
        .await?;
        Ok(details)
    }

    #[allow(dead_code)]
    pub async fn query_non_zero_by_master_id(
        pool: &SqlitePool,
        id: u32,
    ) -> DaoResult<Vec<PlanCurveDetail>> {
        let details = sqlx::query_as::<_, PlanCurveDetail>(
            "SELECT id, curve_id, time_index,
                    CAST(power_value AS REAL) AS power_value,
                    CAST(soc_limit AS REAL) AS soc_limit,
                    created_at, updated_at, deleted_at
            FROM t_plan_curve_detail tpcd
            WHERE tpcd.curve_id = ? AND tpcd.power_value != 0
              AND tpcd.deleted_at is NULL
            ORDER BY tpcd.time_index",
        )
        .bind(id)
        .fetch_all(pool)
        .await?;
        Ok(details)
    }
}
