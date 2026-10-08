use collector_core::runtime::{
    core::get_runtime,
    emu::{PowerGuardConfig, PowerGuardUpdate},
};
use serde::Serialize;

use crate::services::{ServiceError, ServiceResult};

#[derive(Debug, Serialize)]
pub struct SocProtectResp {
    pub charge_limit: f64,
    pub discharge_limit: f64,
}

/// 防逆流与需量保护参数，功率单位 kW
#[derive(Debug, Serialize)]
pub struct PowerGuardResp {
    pub anti_backflow_enable: bool,
    pub anti_backflow_threshold: f64,
    pub anti_backflow_hysteresis: f64,
    pub demand_guard_enable: bool,
    pub demand_limit: f64,
    pub demand_hysteresis: f64,
}

impl From<&PowerGuardConfig> for PowerGuardResp {
    fn from(c: &PowerGuardConfig) -> Self {
        Self {
            anti_backflow_enable: c.anti_backflow_enable(),
            anti_backflow_threshold: c.anti_backflow_threshold(),
            anti_backflow_hysteresis: c.anti_backflow_hysteresis(),
            demand_guard_enable: c.demand_guard_enable(),
            demand_limit: c.demand_limit(),
            demand_hysteresis: c.demand_hysteresis(),
        }
    }
}

pub struct EmuService {}

impl EmuService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {})
    }

    pub async fn soc_protect(&self) -> ServiceResult<SocProtectResp> {
        let runtime = get_runtime()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        Ok(SocProtectResp {
            charge_limit: runtime.emu_runtime.soc_protect.charge_limit(),
            discharge_limit: runtime.emu_runtime.soc_protect.discharge_limit(),
        })
    }

    pub async fn set_soc_protect(
        &self,
        charge_limit: Option<f64>,
        discharge_limit: Option<f64>,
    ) -> ServiceResult<SocProtectResp> {
        let runtime = get_runtime()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        let soc_protect = &runtime.emu_runtime.soc_protect;
        let new_charge_limit = charge_limit.unwrap_or_else(|| soc_protect.charge_limit());
        let new_discharge_limit = discharge_limit.unwrap_or_else(|| soc_protect.discharge_limit());
        if !(0.0..=100.0).contains(&new_charge_limit)
            || !(0.0..=100.0).contains(&new_discharge_limit)
        {
            return Err(ServiceError::InvalidParameter(
                "SOC限制须在0-100之间".to_string(),
            ));
        }
        if new_charge_limit <= new_discharge_limit {
            return Err(ServiceError::InvalidParameter(
                "充电SOC限制须大于放电SOC限制".to_string(),
            ));
        }
        soc_protect.set_charge_limit(new_charge_limit);
        soc_protect.set_discharge_limit(new_discharge_limit);
        runtime
            .emu_runtime
            .save()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        Ok(SocProtectResp {
            charge_limit: new_charge_limit,
            discharge_limit: new_discharge_limit,
        })
    }

    pub async fn power_guard(&self) -> ServiceResult<PowerGuardResp> {
        let runtime = get_runtime()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        Ok(PowerGuardResp::from(&runtime.emu_runtime.power_guard))
    }

    /// 修改防逆流/需量保护参数，只改传入的字段；整体校验通过才生效并落盘
    pub async fn set_power_guard(&self, update: PowerGuardUpdate) -> ServiceResult<PowerGuardResp> {
        let runtime = get_runtime()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        let cfg = &runtime.emu_runtime.power_guard;
        cfg.update(update).map_err(ServiceError::InvalidParameter)?;
        runtime
            .emu_runtime
            .save()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        Ok(PowerGuardResp::from(cfg))
    }
}
