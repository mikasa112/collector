use std::time::Duration;

use async_trait::async_trait;
use collector_core::{
    center::DataCenterError, core::point::ValError, field::FieldError, runtime::RuntimeError,
    utils::taos::TaosDbError,
};

#[derive(Debug, thiserror::Error)]
pub enum StrategyError {
    #[error("{0}")]
    DataCenterErr(#[from] DataCenterError),
    #[error("{0}")]
    RuntimeErr(#[from] RuntimeError),
    #[error("{0}")]
    ValError(#[from] ValError),
    #[error("点`{0}`找不到")]
    PointNotFound(String),
    #[error("{0}")]
    TaosDbError(#[from] TaosDbError),
    #[error("数据库错误: {0}")]
    SqlError(#[from] sqlx::Error),
    #[error("{0}")]
    FieldErr(#[from] FieldError),
}

pub enum Schedule {
    Interval(Duration),
    Cron(String),
}

#[async_trait]
pub trait Strategy: crate::DataDriven + Send + Sync + 'static {
    fn name(&self) -> &str;
    fn schedule(&self) -> Schedule;

    /// 策略需要用到的逻辑字段，供 [`collector_core::field::field_registry`] 注册默认绑定。
    /// EMU 大部分字段已集中在 [`crate::emu::FIELDS`] 里统一注入，这里只用于策略自身
    /// 独有、不适合放进那份集中清单的字段；不需要的策略保持默认空实现即可
    fn fields(&self) -> &'static [collector_core::field::FieldSpec] {
        &[]
    }

    async fn on_start(&mut self) -> Result<(), StrategyError> {
        Ok(())
    }

    async fn on_tick(&mut self) -> Result<(), StrategyError>;
}
