use salvo::{Depot, Request, handler};
use validator::Validate;

use crate::{
    core::{
        ApiResult,
        response::{ListResponse, ObjResponse},
    },
    handlers::RequestExtensions,
    middleware::auth::current_username,
    models::field_binding::PointKind,
    services::{
        ServiceError,
        field_binding::{FieldBindingService, FieldBindingView},
    },
};

#[derive(Debug, serde::Deserialize, Validate)]
pub struct SetFieldBindingParams {
    #[validate(length(min = 1, message = "field_key不能为空"))]
    pub field_key: String,
    #[validate(length(min = 1, message = "dev_id不能为空"))]
    pub dev_id: String,
    pub point_kind: PointKind,
    #[validate(length(min = 1, message = "point_value不能为空"))]
    pub point_value: String,
}

#[handler]
pub async fn list(_req: &mut Request) -> ApiResult<ListResponse<FieldBindingView>> {
    let service = FieldBindingService::new()?;
    let result = service.list().await?;
    let len = result.len();
    Ok(ListResponse::ok(result, len))
}

#[handler]
pub async fn set_binding(req: &mut Request, depot: &mut Depot) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<SetFieldBindingParams>().await?;
    params.validate()?;
    let updated_by = current_username(depot);
    let service = FieldBindingService::new()?;
    service.set_binding(params, updated_by).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn reset_binding(req: &mut Request, depot: &mut Depot) -> ApiResult<ObjResponse<()>> {
    let field_key = RequestExtensions(req)
        .parse_reqeust_parameter::<String>("field_key")
        .ok_or_else(|| ServiceError::InvalidParameter("field_key不能为空".to_string()))?;
    let updated_by = current_username(depot);
    let service = FieldBindingService::new()?;
    service.reset_binding(&field_key, updated_by).await?;
    Ok(ObjResponse::ok(()))
}
