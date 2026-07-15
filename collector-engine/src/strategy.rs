use std::time::Duration;

use async_trait::async_trait;
use collector_core::center::DataCenterError;

#[derive(Debug, thiserror::Error)]
pub enum StrategyError {
    #[error("{0}")]
    DataCenterErr(#[from] DataCenterError),
}

pub enum Schedule {
    Interval(Duration),
    Cron(String),
}

#[async_trait]
pub trait Strategy: crate::DataDriven + Send + Sync + 'static {
    fn name(&self) -> &str;
    fn schedule(&self) -> Schedule;

    async fn on_start(&mut self) -> Result<(), StrategyError> {
        Ok(())
    }

    async fn on_tick(&mut self) -> Result<(), StrategyError>;
}
