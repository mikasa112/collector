use collector_core::runtime::core::get_runtime;
use serde::Serialize;

use crate::services::{ServiceError, ServiceResult};

#[derive(Debug, Serialize)]
pub struct SocProtectResp {
    pub charge_limit: f64,
    pub discharge_limit: f64,
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
        soc_protect
            .save()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        Ok(SocProtectResp {
            charge_limit: new_charge_limit,
            discharge_limit: new_discharge_limit,
        })
    }
}
