pub mod core;
pub mod emu;
pub mod planned_curve;

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("{0}")]
    DbError(#[from] sqlx::Error),
    #[error("找不到表`{0}`")]
    TableNotFound(String),
}
