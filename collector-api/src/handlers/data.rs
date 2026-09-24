use collector_core::center::data_center;
use collector_core::core::point::{PointId, Val};
use salvo::{Request, handler};
use validator::Validate;

use crate::{
    core::{ApiResult, response::ObjResponse},
    services::data::DataService,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Validate)]
pub struct RequestDataParams {
    pub points: Vec<RequestDataParam>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Validate)]
pub struct RequestDataParam {
    #[validate(length(min = 1, message = "设备ID不能为空"))]
    pub dev_id: String,
    pub point_id: Option<PointId>,
    pub point_key: Option<String>,
    pub value: Val,
}

#[handler]
pub async fn set(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<RequestDataParams>().await?;
    params.validate()?;
    let service = DataService::new()?;
    service.set(params).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn list_devices() -> ApiResult<ObjResponse<Vec<String>>> {
    Ok(ObjResponse::ok(data_center().dev_ids()))
}
