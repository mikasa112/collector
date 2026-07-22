use std::time::Duration;

use async_trait::async_trait;
use collector_core::center::SharedPointCenter;

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
        let bcu = self.center.read_many(
            "bms",
            &[
                100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115,
                116, 117, 118, 119, 120, 121,
            ],
        );
        let warnings: Vec<_> = [pcs, tms, bcu]
            .into_iter()
            .flatten()
            .flat_map(|p| p.warning())
            .collect();
        if !warnings.is_empty() {
            tracing::warn!(
                "[故障诊断] {}",
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
    ) -> Result<(), StrategyError> {
        Ok(())
    }
}
