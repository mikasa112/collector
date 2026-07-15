use std::time::Duration;

use async_trait::async_trait;
use collector_core::center::{DataCenterError, SharedPointCenter};

use crate::strategy::{Schedule, Strategy, StrategyError};

pub struct FaultDiagnosis {
    center: SharedPointCenter,
}

impl FaultDiagnosis {
    pub fn new(center: SharedPointCenter) -> Self {
        Self { center }
    }
}

#[async_trait]
impl Strategy for FaultDiagnosis {
    fn name(&self) -> &str {
        "故障诊断"
    }

    fn schedule(&self) -> Schedule {
        Schedule::Interval(Duration::from_secs(3))
    }

    async fn on_tick(&mut self) -> Result<(), StrategyError> {
        let pcs = self
            .center
            .read_many("pcs", &[156, 157, 158, 159, 160, 164, 165]);
        let tms = self.center.read_many("tms", &[20, 21, 22, 23]);

        let warnings: Vec<_> = [pcs, tms]
            .into_iter()
            .flatten()
            .flat_map(|p| p.warning())
            .collect();
        if !warnings.is_empty() {
            tracing::warn!(
                "[故障诊断] {} 条告警: {}",
                warnings.len(),
                warnings.iter().map(|w| w.zh).collect::<Vec<_>>().join(", ")
            );
        }
        Ok(())
    }
}

#[async_trait]
impl crate::DataDriven for FaultDiagnosis {
    async fn down(
        &self,
        _points: &[collector_core::core::point::DownDataPoint],
    ) -> Result<(), DataCenterError> {
        Ok(())
    }
}
