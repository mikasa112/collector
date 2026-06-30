use collector_core::core::point::{PointId, Val};
use salvo::{Depot, Request, handler};
use validator::Validate;

use crate::{
    core::{ApiResult, response::ObjResponse},
    services::data::DataService,
};

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct RequestDataParams {
    pub points: Vec<RequestDataParam>,
}

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct RequestDataParam {
    #[validate(length(min = 1, message = "设备ID不能为空"))]
    pub dev_id: String,
    pub point_id: Option<PointId>,
    pub point_key: Option<String>,
    pub value: Val,
}

#[handler]
pub async fn set(req: &mut Request, depot: &mut Depot) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<RequestDataParams>().await?;
    params.validate()?;
    let service = DataService::new()?;
    service.set(depot, params).await?;
    Ok(ObjResponse::ok(()))
}
