use std::time::Duration;

use async_trait::async_trait;
use collector_core::{
    center::SharedPointCenter,
    core::point::{DataPoint, Val, WarnLevel},
    runtime::{core::get_runtime, emu::HealthStatus},
};

use crate::strategy::{Schedule, Strategy, StrategyError};

pub struct FaultDiagnosis {
    center: SharedPointCenter,
}

impl FaultDiagnosis {
    pub fn new(center: SharedPointCenter) -> Self {
        Self { center }
    }

    /// 将一个带 bits 定义的寄存器展开为若干个单独的告警 DataPoint，
    /// 命中告警的 bit val 置 1，否则置 0；id 留待调用方统一编号
    fn bit_points(point: &DataPoint) -> Vec<DataPoint> {
        let Some(bits) = point.bits else {
            return vec![];
        };
        let Ok(v) = u32::try_from(&point.value) else {
            return vec![];
        };
        bits.bits
            .iter()
            .filter(|it| it.level != WarnLevel::None)
            .enumerate()
            .map(|(i, bit)| DataPoint {
                id: 0,
                key: bit.en,
                name: bit.zh,
                value: Val::U8(((v >> i) & 1) as u8),
                translator: None,
                bits: None,
                words: None,
                unit: None,
            })
            .collect()
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
        let bcu = self.center.read_many(
            "bcu",
            &[
                100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115,
                116, 117, 118, 119, 120, 121,
            ],
        );
        let tms = self.center.read_many("tms", &[20, 21, 22, 23]);
        let mut bit_points: Vec<DataPoint> = pcs
            .iter()
            .chain(bcu.iter())
            .chain(tms.iter())
            .flat_map(Self::bit_points)
            .collect();
        for (i, p) in bit_points.iter_mut().enumerate() {
            p.id = 500 + i as u32;
        }
        self.center.ingest("emu", bit_points);
        let warnings: Vec<_> = [pcs, bcu, tms]
            .into_iter()
            .flatten()
            .flat_map(|p| p.warning())
            .collect();
        let runtime = get_runtime().await?;
        if !warnings.is_empty() {
            //当故障告警不为空，
            for warn in warnings.iter() {
                //2级告警
                if warn.level == WarnLevel::High {
                    runtime.emu_runtime.set_health(HealthStatus::Warning);
                }
                //3级故障
                if warn.level == WarnLevel::Critical {
                    runtime.emu_runtime.set_health(HealthStatus::Alarm);
                    break;
                }
            }
        } else {
            runtime.emu_runtime.set_health(HealthStatus::Normal);
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
