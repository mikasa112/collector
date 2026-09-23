use std::{collections::HashSet, time::Duration};

use async_trait::async_trait;
use collector_core::{
    center::data_center,
    core::point::{DataPoint, Val, WarnLevel},
    runtime::{core::get_runtime, emu::HealthStatus},
};
use sqlx::SqlitePool;

use crate::{
    emu::alarm::{AlaramStatus, AlarmDao},
    strategy::{Schedule, Strategy, StrategyError},
};

pub struct FaultDiagnosis {
    alarm_dao: AlarmDao,
}

/// 一次 tick 中命中的一条故障：dev 为所属设备表名，
/// code 对于打包位告警由 point.id 与 bit 序号组合而成，对于单点告警即 point.id
struct FaultAlarm {
    dev: String,
    code: u32,
    name: &'static str,
    level: WarnLevel,
}

impl FaultDiagnosis {
    pub fn new(pool: SqlitePool) -> Self {
        Self {
            alarm_dao: AlarmDao { pool },
        }
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
                level: None,
            })
            .collect()
    }

    /// 提取寄存器当前命中的故障位，dev 为所属设备表名，
    /// code = point.id * 16 + bit 序号，保证同一 bit 位置每次 tick 算出的 code 一致
    fn fault_alarms(dev: &str, point: &DataPoint) -> Vec<FaultAlarm> {
        let Some(bits) = point.bits else {
            return vec![];
        };
        let Ok(v) = u32::try_from(&point.value) else {
            return vec![];
        };
        bits.bits
            .iter()
            .enumerate()
            .filter(|(_, bit)| bit.level != WarnLevel::None)
            .filter(|(i, _)| (v >> i) & 1 == 1)
            .map(|(i, bit)| FaultAlarm {
                dev: dev.to_string(),
                code: point.id * 16 + i as u32,
                name: bit.zh,
                level: bit.level,
            })
            .collect()
    }

    /// 单点遥信告警：point 配置了 level 且当前值非0即命中，code 直接用 point.id
    /// （同一设备内 id 已在配置构建期校验唯一，不需要再拼 bit 序号）
    fn level_alarms(dev: &str, point: &DataPoint) -> Option<FaultAlarm> {
        let level = point.active_alarm_level()?;
        Some(FaultAlarm {
            dev: dev.to_string(),
            code: point.id,
            name: point.name,
            level,
        })
    }

    /// 将本次 tick 命中的故障与库中仍为 Active 的告警做差集：
    /// 新增的故障 insert 一条 Active 记录，之前 Active 但本次未命中的更新为 Recovered
    async fn sync_alarms(&self, warnings: &[FaultAlarm]) -> Result<(), StrategyError> {
        let current: HashSet<(u32, &str)> =
            warnings.iter().map(|w| (w.code, w.dev.as_str())).collect();
        let active = self.alarm_dao.list_active_keys().await?;

        for w in warnings {
            if !active
                .iter()
                .any(|(code, dev)| *code == w.code && dev == &w.dev)
            {
                self.alarm_dao
                    .create_alarm(
                        w.code,
                        w.name.to_string(),
                        w.dev.clone(),
                        w.level.into(),
                        AlaramStatus::Active,
                        "system".to_string(),
                    )
                    .await?;
            }
        }
        for (code, dev) in active {
            if !current.contains(&(code, dev.as_str())) {
                self.alarm_dao
                    .update_alarm_status(code, dev, AlaramStatus::Recovered)
                    .await?;
            }
        }
        Ok(())
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
        let center = data_center();
        let pcs = center.read_many("pcs", &[156, 157, 158, 159, 160, 164, 165]);
        let bcu = center.read_many(
            "bcu",
            &[
                100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111, 112, 113, 114, 115,
                116, 117, 118, 119, 120, 121,
            ],
        );
        let tms = center.read_many("tms", &[20, 21, 22, 23]);
        let mut bit_points: Vec<DataPoint> = pcs
            .iter()
            .chain(bcu.iter())
            .chain(tms.iter())
            .flat_map(Self::bit_points)
            .collect();
        for (i, p) in bit_points.iter_mut().enumerate() {
            p.id = 500 + i as u32;
        }
        center.ingest("emu", bit_points);
        let bits_warnings: Vec<FaultAlarm> = [("pcs", &pcs), ("bcu", &bcu), ("tms", &tms)]
            .into_iter()
            .flat_map(|(dev, points)| points.iter().flat_map(move |p| Self::fault_alarms(dev, p)))
            .collect();
        // 通用扫描：任意设备里配置了单点告警等级(level)且当前值非0的点位，无需硬编码点位id
        let level_warnings: Vec<FaultAlarm> = center
            .dev_ids()
            .into_iter()
            .flat_map(|dev| {
                center
                    .read_all(&dev)
                    .iter()
                    .filter_map(|p| Self::level_alarms(&dev, p))
                    .collect::<Vec<_>>()
            })
            .collect();
        let warnings: Vec<FaultAlarm> = bits_warnings.into_iter().chain(level_warnings).collect();
        self.sync_alarms(&warnings).await?;
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
