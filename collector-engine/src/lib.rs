use collector_core::core::point::DownDataPoint;

use crate::strategy::StrategyError;

pub mod emu;
pub mod mod_engine;
pub mod strategy;

#[async_trait::async_trait]
pub trait DataDriven {
    async fn down(&self, points: &[DownDataPoint]) -> Result<(), StrategyError>;
}
