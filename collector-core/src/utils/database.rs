use sqlx::ConnectOptions;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};
use std::sync::OnceLock;
use std::time::Duration;

/// 全局数据库连接池
static DB_POOL: OnceLock<SqlitePool> = OnceLock::new();

#[derive(Debug, thiserror::Error)]
pub enum DatabaseError {
    #[error("io error: {0}")]
    IoError(#[from] tokio::io::Error),
    #[error("sqlx error: {0}")]
    SqlxError(#[from] sqlx::Error),
    #[error("conn pool already initialized")]
    ConnPoolAlreadyInitialized,
    #[error("conn pool not initialized")]
    ConnPoolNotInitialized,
}

/// 数据库配置
#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    /// 数据库文件路径
    pub path: String,
    /// 最大连接数
    pub max_connections: u32,
    /// 最小连接数
    pub min_connections: u32,
    /// 连接超时时间（秒）
    pub connect_timeout: u64,
    /// 空闲连接超时时间（秒）
    pub idle_timeout: u64,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: "./data.db".to_string(),
            max_connections: 10,
            min_connections: 2,
            connect_timeout: 30,
            idle_timeout: 600,
        }
    }
}

/// 初始化数据库连接池
pub async fn init_database(config: DatabaseConfig) -> Result<SqlitePool, DatabaseError> {
    // 确保数据库目录存在
    if let Some(parent) = std::path::Path::new(&config.path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // 配置 SQLite 连接选项
    let options = SqliteConnectOptions::new()
        .filename(&config.path)
        .create_if_missing(true)
        .disable_statement_logging();

    // 创建连接池
    let pool = SqlitePoolOptions::new()
        .max_connections(config.max_connections)
        .min_connections(config.min_connections)
        .acquire_timeout(Duration::from_secs(config.connect_timeout))
        .idle_timeout(Duration::from_secs(config.idle_timeout))
        .connect_with(options)
        .await?;

    // 设置全局连接池
    DB_POOL
        .set(pool.clone())
        .map_err(|_| DatabaseError::ConnPoolAlreadyInitialized)?;

    Ok(pool)
}

/// 获取数据库连接池
pub fn get_database() -> Result<SqlitePool, DatabaseError> {
    let pool = DB_POOL
        .get()
        .cloned()
        .ok_or_else(|| DatabaseError::ConnPoolNotInitialized)?;
    Ok(pool)
}

/// 关闭数据库连接池
pub async fn close_database() {
    if let Some(pool) = DB_POOL.get() {
        pool.close().await;
    }
}
