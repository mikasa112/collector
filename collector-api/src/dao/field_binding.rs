use sqlx::SqlitePool;

use crate::{
    dao::error::DaoResult,
    models::field_binding::{FieldBindingOverride, PointKind},
};

pub struct FieldBindingDao;

/// 新建/更新字段绑定覆盖记录所需参数
pub struct UpsertFieldBinding<'a> {
    pub field_key: &'a str,
    pub dev_id: &'a str,
    pub point_kind: PointKind,
    pub point_value: &'a str,
    pub updated_by: Option<&'a str>,
}

impl FieldBindingDao {
    pub async fn find_all_enabled(pool: &SqlitePool) -> DaoResult<Vec<FieldBindingOverride>> {
        let list = sqlx::query_as::<_, FieldBindingOverride>(
            "SELECT * FROM t_field_binding_override WHERE enabled = 1 ORDER BY field_key",
        )
        .fetch_all(pool)
        .await?;
        Ok(list)
    }

    /// 覆盖写入某个字段的绑定，已存在(含已重置)的记录直接更新并重新启用
    pub async fn upsert(pool: &SqlitePool, params: UpsertFieldBinding<'_>) -> DaoResult<u64> {
        let result = sqlx::query(
            "INSERT INTO t_field_binding_override (field_key, dev_id, point_kind, point_value, enabled, updated_by)
             VALUES (?, ?, ?, ?, 1, ?)
             ON CONFLICT (field_key) DO UPDATE SET
                 dev_id = excluded.dev_id,
                 point_kind = excluded.point_kind,
                 point_value = excluded.point_value,
                 enabled = 1,
                 updated_by = excluded.updated_by,
                 updated_at = datetime('now', 'localtime')",
        )
        .bind(params.field_key)
        .bind(params.dev_id)
        .bind(params.point_kind)
        .bind(params.point_value)
        .bind(params.updated_by)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }

    /// 重置为默认绑定：仅对当前生效中的覆盖记录置为禁用
    pub async fn reset(
        pool: &SqlitePool,
        field_key: &str,
        updated_by: Option<&str>,
    ) -> DaoResult<u64> {
        let result = sqlx::query(
            "UPDATE t_field_binding_override
             SET enabled = 0,
                 updated_by = ?,
                 updated_at = datetime('now', 'localtime')
             WHERE field_key = ?
               AND enabled = 1",
        )
        .bind(updated_by)
        .bind(field_key)
        .execute(pool)
        .await?;
        Ok(result.rows_affected())
    }
}
