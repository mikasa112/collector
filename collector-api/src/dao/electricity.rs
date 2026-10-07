use sqlx::SqlitePool;

use crate::{
    dao::error::DaoResult,
    models::electricity::{ElectricityPeriod, ElectricityPrice, PeriodType},
};

pub struct ElectricityDao;

impl ElectricityDao {
    pub async fn find_prices(pool: &SqlitePool) -> DaoResult<Vec<ElectricityPrice>> {
        let list = sqlx::query_as::<_, ElectricityPrice>(
            // DECIMAL 列是 NUMERIC 亲和性，整数值(如 1)会以 INTEGER 存储，需转成 REAL 才能解码为 f64
            "SELECT period_type, CAST(price AS REAL) AS price FROM t_electricity_price
             ORDER BY period_type",
        )
        .fetch_all(pool)
        .await?;
        Ok(list)
    }

    /// 同一事务内按 period_type 覆盖写入若干单价
    pub async fn upsert_prices(
        pool: &SqlitePool,
        prices: &[(PeriodType, f64)],
        updated_by: Option<&str>,
    ) -> DaoResult<()> {
        let mut tx = pool.begin().await?;
        for (period_type, price) in prices {
            sqlx::query(
                "INSERT INTO t_electricity_price (period_type, price, updated_by) VALUES (?, ?, ?)
                 ON CONFLICT (period_type) DO UPDATE SET
                     price = excluded.price,
                     updated_by = excluded.updated_by,
                     updated_at = datetime('now', 'localtime')",
            )
            .bind(period_type)
            .bind(price)
            .bind(updated_by)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn find_periods(pool: &SqlitePool) -> DaoResult<Vec<ElectricityPeriod>> {
        let list = sqlx::query_as::<_, ElectricityPeriod>(
            "SELECT start_time, end_time, period_type FROM t_electricity_period
             ORDER BY start_time",
        )
        .fetch_all(pool)
        .await?;
        Ok(list)
    }

    /// 同一事务内清空并重写全部时段，避免中途失败留下残缺的时段表
    pub async fn replace_periods(
        pool: &SqlitePool,
        periods: &[ElectricityPeriod],
        updated_by: Option<&str>,
    ) -> DaoResult<()> {
        let mut tx = pool.begin().await?;
        sqlx::query("DELETE FROM t_electricity_period")
            .execute(&mut *tx)
            .await?;
        for p in periods {
            sqlx::query(
                "INSERT INTO t_electricity_period (start_time, end_time, period_type, updated_by)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&p.start_time)
            .bind(&p.end_time)
            .bind(p.period_type)
            .bind(updated_by)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
