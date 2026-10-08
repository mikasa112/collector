use std::time::Duration;

use collector_core::{
    center::data_center,
    core::point::{DataPoint, DownDataPoint, Val},
    field::field_registry,
    runtime::{
        core::get_runtime,
        emu::{ControlSource, EmuPermission, OperationMode, RunMode},
    },
};

use crate::{
    DataDriven,
    emu::{
        CHARGE_SOC_LIMIT, CONTROL_SOURCE, DISCHARGE_SOC_LIMIT, HEALTH_STATUS, OPERATION_MODE,
        PERMISSION, RUN_MODE,
    },
    strategy::{Schedule, Strategy, StrategyError},
};

pub struct EmuRuntime {}

impl EmuRuntime {
    pub fn new() -> Self {
        Self {}
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
        let center = data_center();
        let state = field_registry().read("bcu_comm_status")?;
        let soc = field_registry()
            .read("soc")?
            .map(|it| it.value.as_f64().unwrap_or(0.0))
            .unwrap_or(0.0);
        //电流负充正放
        let bcu_current = field_registry()
            .read("bcu_current")?
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
            if bcu_current < 0.0 {
                let _ = field_registry()
                    .dispatch("pcs_active_power", Val::F64(0.0))
                    .await;
                tracing::warn!("[EMU] 系统禁充, 修正有功功率为0")
            }
            EmuPermission::ChargeDisabled
        } else if soc <= discharge_limit {
            if bcu_current > 0.0 {
                let _ = field_registry()
                    .dispatch("pcs_active_power", Val::F64(0.0))
                    .await;
                tracing::warn!("[EMU] 系统禁放, 修正有功功率为0")
            }
            EmuPermission::DischargeDisabled
        } else {
            EmuPermission::Normal
        };
        runtime.emu_runtime.set_permission(per);
        let h = runtime
            .emu_runtime
            .health()
            .unwrap_or(collector_core::runtime::emu::HealthStatus::Alarm);
        //计划自动模式依赖计划曲线使能，计划曲线被关闭时自动切换为总功率模式
        let rm = runtime
            .emu_runtime
            .run_mode()
            .unwrap_or(RunMode::TotalPower);
        let rm = if matches!(rm, RunMode::PlanAuto)
            && !runtime.planned_curve.get_planned_curve_enable()
        {
            runtime.emu_runtime.set_run_mode(RunMode::TotalPower);
            tracing::warn!("[EMU] 计划曲线已关闭, 运行模式自动切换为总功率");
            RunMode::TotalPower
        } else {
            rm
        };
        let cs = runtime
            .emu_runtime
            .control_source()
            .unwrap_or(ControlSource::Local);
        center.ingest(
            "emu",
            vec![
                operation_mode(s as u8),
                permission(per as u8),
                health_status(h as u8),
                charge_soc_limit(charge_limit),
                discharge_soc_limit(discharge_limit),
                run_mode(rm as u8),
                control_source(cs as u8),
            ],
        );
        Ok(())
    }
}

#[async_trait::async_trait]
impl DataDriven for EmuRuntime {
    async fn down(&self, points: &[DownDataPoint]) -> Result<(), StrategyError> {
        let runtime = get_runtime().await?;
        let mut changed = false;
        for p in points.iter() {
            if CHARGE_SOC_LIMIT.matches(&p.point) {
                runtime
                    .emu_runtime
                    .soc_protect
                    .set_charge_limit(p.value.as_f64()?);
                tracing::info!("[EMU] 充电SOC限制修改为{}", p.value);
                changed = true;
            }
            if DISCHARGE_SOC_LIMIT.matches(&p.point) {
                runtime
                    .emu_runtime
                    .soc_protect
                    .set_discharge_limit(p.value.as_f64()?);
                tracing::info!("[EMU] 放电SOC限制修改为{}", p.value);
                changed = true;
            }
            if RUN_MODE.matches(&p.point) {
                let Ok(mode) = RunMode::try_from(p.value.as_u32()? as u8) else {
                    tracing::warn!("[EMU] 无效的运行模式取值: {}", p.value);
                    continue;
                };
                if matches!(mode, RunMode::PlanAuto)
                    && !runtime.planned_curve.get_planned_curve_enable()
                {
                    tracing::warn!("[EMU] 计划曲线未开启, 无法切换为计划自动模式");
                } else {
                    runtime.emu_runtime.set_run_mode(mode);
                    tracing::info!("[EMU] 运行模式修改为{}", p.value);
                }
            }
            if CONTROL_SOURCE.matches(&p.point) {
                let Ok(source) = ControlSource::try_from(p.value.as_u32()? as u8) else {
                    tracing::warn!("[EMU] 无效的控制源取值: {}", p.value);
                    continue;
                };
                runtime.emu_runtime.set_control_source(source);
                tracing::info!("[EMU] 控制源修改为{}", p.value);
            }
        }
        if changed && let Err(err) = runtime.emu_runtime.save().await {
            tracing::error!("[EMU] 保存SOC保护配置失败: {}", err);
        }
        Ok(())
    }
}
fn operation_mode(data: u8) -> DataPoint {
    DataPoint {
        id: OPERATION_MODE.id,
        key: OPERATION_MODE.key,
        name: "EMU充放电状态",
        value: Val::U8(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
        level: None,
    }
}

fn permission(data: u8) -> DataPoint {
    DataPoint {
        id: PERMISSION.id,
        key: PERMISSION.key,
        name: "EMU充放电许可",
        value: Val::U8(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
        level: None,
    }
}

fn health_status(data: u8) -> DataPoint {
    DataPoint {
        id: HEALTH_STATUS.id,
        key: HEALTH_STATUS.key,
        name: "EMU告警故障状态",
        value: Val::U8(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
        level: None,
    }
}

fn charge_soc_limit(data: f64) -> DataPoint {
    DataPoint {
        id: CHARGE_SOC_LIMIT.id,
        key: CHARGE_SOC_LIMIT.key,
        name: "充电SOC限制",
        value: Val::F64(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
        level: None,
    }
}

fn run_mode(data: u8) -> DataPoint {
    DataPoint {
        id: RUN_MODE.id,
        key: RUN_MODE.key,
        name: "EMU运行模式",
        value: Val::U8(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
        level: None,
    }
}

fn control_source(data: u8) -> DataPoint {
    DataPoint {
        id: CONTROL_SOURCE.id,
        key: CONTROL_SOURCE.key,
        name: "EMU控制源",
        value: Val::U8(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
        level: None,
    }
}

fn discharge_soc_limit(data: f64) -> DataPoint {
    DataPoint {
        id: DISCHARGE_SOC_LIMIT.id,
        key: DISCHARGE_SOC_LIMIT.key,
        name: "放电SOC限制",
        value: Val::F64(data),
        translator: None,
        bits: None,
        words: None,
        unit: None,
        level: None,
    }
}
