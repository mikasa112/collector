use sqlx::SqlitePool;

use crate::dao::error::DaoResult;

pub struct ProjectInfoDao;

impl ProjectInfoDao {
    pub async fn find_all(pool: &SqlitePool) -> DaoResult<Vec<(String, String)>> {
        let list = sqlx::query_as::<_, (String, String)>("SELECT key, value FROM t_project_info")
            .fetch_all(pool)
            .await?;
        Ok(list)
    }

    /// 在同一事务中写入/删除多个键：`Some(value)` 为覆盖写入，`None` 为清除
    pub async fn apply(
        pool: &SqlitePool,
        changes: &[(&str, Option<String>)],
        updated_by: Option<&str>,
    ) -> DaoResult<()> {
        let mut tx = pool.begin().await?;
        for (key, value) in changes {
            match value {
                Some(value) => {
                    sqlx::query(
                        "INSERT INTO t_project_info (key, value, updated_by) VALUES (?, ?, ?)
                         ON CONFLICT (key) DO UPDATE SET
                             value = excluded.value,
                             updated_by = excluded.updated_by,
                             updated_at = datetime('now', 'localtime')",
                    )
                    .bind(key)
                    .bind(value)
                    .bind(updated_by)
                    .execute(&mut *tx)
                    .await?;
                }
                None => {
                    sqlx::query("DELETE FROM t_project_info WHERE key = ?")
                        .bind(key)
                        .execute(&mut *tx)
                        .await?;
                }
            }
        }
        tx.commit().await?;
        Ok(())
    }
}
