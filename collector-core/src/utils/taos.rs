use std::sync::OnceLock;

use futures::TryStreamExt;
use taos::{AsyncFetchable, AsyncQueryable, AsyncTBuilder, Taos, TaosBuilder};

#[derive(Debug, thiserror::Error)]
pub enum TaosDbError {
    #[error("taos error: {0}")]
    TaosError(#[from] taos::Error),
    #[error("conn pool already initialized")]
    ConnPoolAlreadyInitialized,
    #[error("conn pool not initialized")]
    ConnPoolNotInitialized,
}

static TAOS_POOL: OnceLock<Taos> = OnceLock::new();

pub async fn init_taos() -> Result<(), TaosDbError> {
    let dsn = "ws://localhost:6041";
    let taos = TaosBuilder::from_dsn(dsn)?.build().await?;
    TAOS_POOL
        .set(taos)
        .map_err(|_| TaosDbError::ConnPoolAlreadyInitialized)?;
    Ok(())
}

pub fn get_taos() -> Result<&'static Taos, TaosDbError> {
    let pool = TAOS_POOL
        .get()
        .ok_or_else(|| TaosDbError::ConnPoolNotInitialized)?;
    Ok(pool)
}

/// 执行查询 SQL，并将结果按行反序列化为 `T`（字段名需与 SQL 结果列名/别名一致）
pub async fn query_rows<T>(sql: &str) -> Result<Vec<T>, TaosDbError>
where
    T: serde::de::DeserializeOwned,
{
    let taos = get_taos()?;
    let mut rs = taos.query(sql).await?;
    let rows: Vec<T> = rs.deserialize::<T>().try_collect().await?;
    Ok(rows)
}
