use std::time::Duration;

use collector_core::{
    center::SharedPointCenter,
    core::point::{DataPoint, DownDataPoint, Val},
    runtime::{
        core::get_runtime,
        emu::{EmuPermission, OperationMode},
    },
};

use crate::{
    DataDriven,
    emu::{
        ID_CHARGE_SOC_LIMIT, ID_DISCHARGE_SOC_LIMIT, ID_HEALTH_STATUS, ID_OPERATION_MODE,
        ID_PERMISSION, KEY_CHARGE_SOC_LIMIT, KEY_DISCHARGE_SOC_LIMIT, KEY_HEALTH_STATUS,
        KEY_OPERATION_MODE, KEY_PERMISSION,
    },
    strategy::{Schedule, Strategy, StrategyError},
};
pub struct EmuRuntime {
    center: SharedPointCenter,
}

impl EmuRuntime {
    pub fn new(center: SharedPointCenter) -> Self {
        Self { center }
    }
}

#[async_trait::async_trait]
impl Strategy for EmuRuntime {
    fn name(&self) -> &str {
        "EMU运行时策略"
    }

    fn schedule(&self) -> Schedule {
        Schedule::Interval(Duration::from_secs(1))
    }

    async fn on_start(&mut self) -> Result<(), StrategyError> {
        Ok(())
    }

    async fn on_tick(&mut self) -> Result<(), StrategyError> {
        let state = self.center.read("bcu", 34);
        let soc = self
            .center
            .read("bcu", 32)
            .map(|it| it.value.as_f64().unwrap_or(0.0))
            .unwrap_or(0.0);
        let state = state.map(|it| it.value.as_u32().unwrap_or(0)).unwrap_or(0);
        let s = match state {
            0 => OperationMode::Standby,
            1 => OperationMode::Discharging,
            2 => OperationMode::Charging,
            _ => OperationMode::Standby,
        };
        let runtime = get_runtime().await?;
        runtime.emu_runtime.set_operation_mode(s);
        let charge_limit = runtime.emu_runtime.soc_protect.charge_limit();
        let discharge_limit = runtime.emu_runtime.soc_protect.discharge_limit();
        let per = if soc >= charge_limit {
            EmuPermission::ChargeDisabled
        } else if soc <= discharge_limit {
            EmuPermission::DischargeDisabled
        } else {
            EmuPermission::Normal
        };
        runtime.emu_runtime.set_permission(per);
        let h = runtime
            .emu_runtime
            .health()
            .unwrap_or(collector_core::runtime::emu::HealthStatus::Alarm);
        self.center.ingest(
            "emu",
            vec![
                operation_mode(s as u8),
                permission(per as u8),
                health_status(h as u8),
            ],
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl DataDriven for EmuRuntime {
    async fn down(&self, _points: &[DownDataPoint]) -> Result<(), StrategyError> {
        Ok(())
    }
}
fn operation_mode(data: u8) -> DataPoint {
    DataPoint {
        id: ID_OPERATION_MODE,
        key: KEY_OPERATION_MODE,
        name: "EMU运行模式",
        value: Val::U8(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
    }
}

fn permission(data: u8) -> DataPoint {
    DataPoint {
        id: ID_PERMISSION,
        key: KEY_PERMISSION,
        name: "EMU充放电许可",
        value: Val::U8(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
    }
}

fn health_status(data: u8) -> DataPoint {
    DataPoint {
        id: ID_HEALTH_STATUS,
        key: KEY_HEALTH_STATUS,
        name: "EMU告警故障状态",
        value: Val::U8(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
    }
}

fn charge_soc_limit(data: f64) -> DataPoint {
    DataPoint {
        id: ID_CHARGE_SOC_LIMIT,
        key: KEY_CHARGE_SOC_LIMIT,
        name: "充电SOC限制",
        value: Val::F64(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
    }
}

fn discharge_soc_limit(data: f64) -> DataPoint {
    DataPoint {
        id: ID_DISCHARGE_SOC_LIMIT,
        key: KEY_DISCHARGE_SOC_LIMIT,
        name: "放电SOC限制",
        value: Val::F64(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
    }
}
