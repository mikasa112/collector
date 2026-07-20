use salvo::{Request, handler};
use validator::Validate;

use crate::{
    core::{
        ApiResult,
        response::{ListResponse, ObjResponse},
    },
    handlers::RequestExtensions,
    models::planned_curve::{CurveType, PlanCurveDetail},
    services::{
        ServiceError,
        planned_curve::{PlanCurveMasterSimpleResp, PlannedCurveService},
    },
};

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct CreatePlannedCurveParams {
    #[validate(length(min = 1, message = "curve_name不能为空"))]
    pub curve_name: String,
    #[validate(required(message = "curve_type不能为空"))]
    pub curve_type: Option<CurveType>,
    pub priority: Option<u8>,
    pub status: Option<u8>,
    pub valid_start_date: Option<String>,
    pub valid_end_date: Option<String>,
    pub effective_weekdays: Option<String>,
    pub created_by: Option<String>,
    pub remark: Option<String>,
}

#[derive(serde::Deserialize, Debug)]
pub struct TimesDetailsParams {
    pub time_index: u8,
    pub power_value: f64,
    pub soc_limit: Option<f64>,
}

#[derive(Debug, serde::Deserialize, Validate)]
pub struct BindPlannedCurveDetailsParams {
    pub curve_id: u32,
    pub times: Vec<TimesDetailsParams>,
}

#[handler]
pub async fn list(req: &mut Request) -> ApiResult<ListResponse<PlanCurveMasterSimpleResp>> {
    let page = req.query::<u32>("page").unwrap_or(1);
    let size = req.query::<u32>("size").unwrap_or(10);
    let service = PlannedCurveService::new()?;
    let (vec, total) = service.planned_curve_list(page, size).await?;
    Ok(ListResponse::ok(vec, total))
}

#[handler]
pub async fn find_master_by_id(
    req: &mut Request,
) -> ApiResult<ObjResponse<PlanCurveMasterSimpleResp>> {
    let service = PlannedCurveService::new()?;
    let id = RequestExtensions(req)
        .parse_reqeust_parameter::<u32>("id")
        .ok_or_else(|| ServiceError::InvalidParameter("id不能为空".to_string()))?;
    let result = service.find_master_by_id(id).await?;
    Ok(ObjResponse::ok(result))
}

#[handler]
pub async fn create_planned_curve_master(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<CreatePlannedCurveParams>().await?;
    params.validate()?;
    let service = PlannedCurveService::new()?;
    service.create_planned_curve_master(params).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn bind_planned_curve_details(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<BindPlannedCurveDetailsParams>().await?;
    params.validate()?;
    let service = PlannedCurveService::new()?;
    service.bind_planned_curve_details(params).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn planned_curve_details(req: &mut Request) -> ApiResult<ListResponse<PlanCurveDetail>> {
    let curve_id = RequestExtensions(req)
        .parse_reqeust_parameter::<u32>("curve_id")
        .ok_or_else(|| ServiceError::InvalidParameter("curve_id不能为空".to_string()))?;
    let service = PlannedCurveService::new()?;
    let result = service.planned_curve_details(curve_id).await?;
    let len = result.len();
    Ok(ListResponse::ok(result, len))
}
