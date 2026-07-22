use std::sync::atomic::AtomicBool;

use sqlx::Row;

use crate::runtime::RuntimeError;

pub struct RuntimePlannedCurve {
    planned_curve_enable: AtomicBool,
    pool: sqlx::SqlitePool,
}
impl RuntimePlannedCurve {
    pub async fn new(pool: sqlx::SqlitePool) -> Result<Self, RuntimeError> {
        let enable = read_planed_curve_enable(&pool).await?;
        Ok(Self {
            planned_curve_enable: AtomicBool::new(enable),
            pool,
        })
    }

    pub async fn set_planned_curve_enable(&self, enable: bool) -> Result<(), RuntimeError> {
        let enable = if enable { 1 } else { 0 };
        let sql_data = sqlx::query(
            "UPDATE t_emu_function
             SET enabled = ?,
                 updated_at = datetime('now', 'localtime')
             WHERE function_code = 'PLAN_CURVE'
             AND deleted_at IS NULL;",
        )
        .bind(enable)
        .execute(&self.pool)
        .await?;
        if sql_data.rows_affected() == 0 {
            return Err(RuntimeError::TableNotFound("t_emu_function".to_string()));
        }
        self.planned_curve_enable
            .store(enable == 1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub async fn get_planned_curve_enable(&self) -> bool {
        self.planned_curve_enable
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

async fn read_planed_curve_enable(pool: &sqlx::SqlitePool) -> Result<bool, RuntimeError> {
    let data = sqlx::query(
        "SELECT function_code, function_name, enabled
         FROM t_emu_function
         WHERE function_code = 'PLAN_CURVE'
           AND deleted_at IS NULL;",
    )
    .fetch_optional(pool)
    .await?;
    let enable = match data {
        Some(row) => row.try_get("enabled")?,
        None => false,
    };
    Ok(enable)
}
