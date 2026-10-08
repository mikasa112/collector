use collector_core::runtime::emu::PowerGuardUpdate;
use salvo::{Request, handler};
use validator::Validate;

use crate::{
    core::{ApiResult, response::ObjResponse},
    services::emu::{EmuService, PowerGuardResp, SocProtectResp},
};

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct SetSocProtectParams {
    #[validate(range(min = 0.0, max = 100.0, message = "充电SOC限制须在0-100之间"))]
    pub charge_limit: Option<f64>,
    #[validate(range(min = 0.0, max = 100.0, message = "放电SOC限制须在0-100之间"))]
    pub discharge_limit: Option<f64>,
}

/// 防逆流/需量保护参数修改，字段均可选，只修改传入的字段。
/// 回差与限值之间的关联约束（0 < 需量回差 < 需量限制）由服务层整体校验
#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct SetPowerGuardParams {
    pub anti_backflow_enable: Option<bool>,
    #[validate(range(min = 0.0, message = "防逆流功率阈值须大于等于0"))]
    pub anti_backflow_threshold: Option<f64>,
    #[validate(range(min = 0.0, message = "防逆流功率回差须大于0"))]
    pub anti_backflow_hysteresis: Option<f64>,
    pub demand_guard_enable: Option<bool>,
    #[validate(range(min = 0.0, message = "需量限制须大于0"))]
    pub demand_limit: Option<f64>,
    #[validate(range(min = 0.0, message = "需量保护功率回差须大于0"))]
    pub demand_hysteresis: Option<f64>,
}

#[handler]
pub async fn soc_protect() -> ApiResult<ObjResponse<SocProtectResp>> {
    let service = EmuService::new()?;
    let result = service.soc_protect().await?;
    Ok(ObjResponse::ok(result))
}

#[handler]
pub async fn set_soc_protect(req: &mut Request) -> ApiResult<ObjResponse<SocProtectResp>> {
    let params = req.parse_json::<SetSocProtectParams>().await?;
    params.validate()?;
    let service = EmuService::new()?;
    let result = service
        .set_soc_protect(params.charge_limit, params.discharge_limit)
        .await?;
    Ok(ObjResponse::ok(result))
}

#[handler]
pub async fn power_guard() -> ApiResult<ObjResponse<PowerGuardResp>> {
    let service = EmuService::new()?;
    let result = service.power_guard().await?;
    Ok(ObjResponse::ok(result))
}

#[handler]
pub async fn set_power_guard(req: &mut Request) -> ApiResult<ObjResponse<PowerGuardResp>> {
    let params = req.parse_json::<SetPowerGuardParams>().await?;
    params.validate()?;
    let service = EmuService::new()?;
    let result = service
        .set_power_guard(PowerGuardUpdate {
            anti_backflow_enable: params.anti_backflow_enable,
            anti_backflow_threshold: params.anti_backflow_threshold,
            anti_backflow_hysteresis: params.anti_backflow_hysteresis,
            demand_guard_enable: params.demand_guard_enable,
            demand_limit: params.demand_limit,
            demand_hysteresis: params.demand_hysteresis,
        })
        .await?;
    Ok(ObjResponse::ok(result))
}
