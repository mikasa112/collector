use salvo::{Request, handler};
use validator::Validate;

use crate::{
    core::{ApiResult, response::ObjResponse},
    services::emu::{EmuService, SocProtectResp},
};

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct SetSocProtectParams {
    #[validate(range(min = 0.0, max = 100.0, message = "充电SOC限制须在0-100之间"))]
    pub charge_limit: Option<f64>,
    #[validate(range(min = 0.0, max = 100.0, message = "放电SOC限制须在0-100之间"))]
    pub discharge_limit: Option<f64>,
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
