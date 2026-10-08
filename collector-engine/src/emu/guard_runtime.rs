//! 防逆流/需量保护的运行时：参数点位的读写、回读，以及保护状态的周期复评。
//!
//! 拦截器 [`PowerDispatchGuard`] 只在有下发时才计算钳位，如果上游只下发一次功率，
//! 之后负荷变化造成逆流/超需量就没有人再触发它。因此这里每秒复评一次：
//! 用最近一次上游请求值重新求放行值，变化超过死区才重新走一遍下发。

use std::{sync::Arc, time::Duration};

use collector_core::{
    center::data_center,
    core::point::{DataPoint, DownDataPoint, Val},
    field::field_registry,
    runtime::{core::get_runtime, emu::PowerGuardConfig},
};

use crate::{
    DataDriven,
    emu::{
        ANTI_BACKFLOW_ENABLE, ANTI_BACKFLOW_HYSTERESIS, ANTI_BACKFLOW_THRESHOLD,
        DEMAND_GUARD_ENABLE, DEMAND_HYSTERESIS, DEMAND_LIMIT, Point,
        power_guard::PowerDispatchGuard,
    },
    strategy::{Schedule, Strategy, StrategyError},
};

pub struct PowerGuardRuntime {
    guard: Arc<PowerDispatchGuard>,
    cfg: &'static PowerGuardConfig,
}

impl PowerGuardRuntime {
    pub fn new(guard: Arc<PowerDispatchGuard>, cfg: &'static PowerGuardConfig) -> Self {
        Self { guard, cfg }
    }
}

#[async_trait::async_trait]
impl Strategy for PowerGuardRuntime {
    fn name(&self) -> &str {
        "功率保护"
    }

    fn schedule(&self) -> Schedule {
        Schedule::Interval(Duration::from_secs(1))
    }

    async fn on_tick(&mut self) -> Result<(), StrategyError> {
        let cfg = &self.cfg;
        data_center().ingest(
            "emu",
            vec![
                flag(
                    &ANTI_BACKFLOW_ENABLE,
                    "防逆流使能",
                    cfg.anti_backflow_enable(),
                ),
                value(
                    &ANTI_BACKFLOW_THRESHOLD,
                    "防逆流功率阈值",
                    cfg.anti_backflow_threshold(),
                ),
                value(
                    &ANTI_BACKFLOW_HYSTERESIS,
                    "防逆流功率回差",
                    cfg.anti_backflow_hysteresis(),
                ),
                flag(
                    &DEMAND_GUARD_ENABLE,
                    "需量保护使能",
                    cfg.demand_guard_enable(),
                ),
                value(&DEMAND_LIMIT, "需量限制", cfg.demand_limit()),
                value(
                    &DEMAND_HYSTERESIS,
                    "需量保护功率回差",
                    cfg.demand_hysteresis(),
                ),
            ],
        );

        if let Some(requested) = self.guard.pending_redispatch(data_center()) {
            tracing::info!("[功率保护] 工况变化, 重新下发有功功率请求值 {requested}");
            field_registry()
                .dispatch(self.guard.field_key(), Val::F64(requested))
                .await?;
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl DataDriven for PowerGuardRuntime {
    async fn down(&self, points: &[DownDataPoint]) -> Result<(), StrategyError> {
        let cfg = &self.cfg;
        let mut changed = false;
        for p in points {
            let result = if ANTI_BACKFLOW_ENABLE.matches(&p.point) {
                cfg.set_anti_backflow_enable(p.value.as_u32()? != 0);
                Ok(())
            } else if ANTI_BACKFLOW_THRESHOLD.matches(&p.point) {
                cfg.set_anti_backflow_threshold(p.value.as_f64()?)
            } else if ANTI_BACKFLOW_HYSTERESIS.matches(&p.point) {
                cfg.set_anti_backflow_hysteresis(p.value.as_f64()?)
            } else if DEMAND_GUARD_ENABLE.matches(&p.point) {
                cfg.set_demand_guard_enable(p.value.as_u32()? != 0);
                Ok(())
            } else if DEMAND_LIMIT.matches(&p.point) {
                cfg.set_demand_limit(p.value.as_f64()?)
            } else if DEMAND_HYSTERESIS.matches(&p.point) {
                cfg.set_demand_hysteresis(p.value.as_f64()?)
            } else {
                continue;
            };
            match result {
                Ok(()) => {
                    tracing::info!("[功率保护] 参数修改为{}", p.value);
                    changed = true;
                }
                Err(err) => tracing::warn!("[功率保护] 参数被拒绝: {err}"),
            }
        }
        if changed {
            let runtime = get_runtime().await?;
            if let Err(err) = runtime.emu_runtime.save().await {
                tracing::error!("[功率保护] 保存配置失败: {}", err);
            }
        }
        Ok(())
    }
}

fn data_point(
    point: &Point,
    name: &'static str,
    value: Val,
    unit: Option<&'static str>,
) -> DataPoint {
    DataPoint {
        id: point.id,
        key: point.key,
        name,
        value,
        translator: None,
        bits: None,
        words: None,
        unit,
        level: None,
    }
}

fn flag(point: &Point, name: &'static str, on: bool) -> DataPoint {
    data_point(point, name, Val::U8(on as u8), None)
}

fn value(point: &Point, name: &'static str, v: f64) -> DataPoint {
    data_point(point, name, Val::F64(v), Some("kW"))
}
