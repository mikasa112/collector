use salvo::{Request, handler};

use crate::{
    core::{
        ApiResult,
        response::{ListResponse, ObjResponse},
    },
    models::planned_curve::PlanCurveMaster,
    services::planned_curve::PlannedCurveService,
};

#[handler]
pub async fn list(req: &mut Request) -> ApiResult<ListResponse<PlanCurveMaster>> {
    let page = req.query::<u32>("page").unwrap_or(1);
    let size = req.query::<u32>("size").unwrap_or(10);
    let service = PlannedCurveService::new()?;
    let (vec, total) = service.planned_curve_list(page, size).await?;
    Ok(ListResponse::ok(vec, total))
}

#[handler]
pub async fn find_master_by_id(req: &mut Request) -> ApiResult<ObjResponse<PlanCurveMaster>> {
    let service = PlannedCurveService::new()?;
    let id = req.params().get("id");
    // Ok(ObjResponse::ok(service.find_master_by_id(id)?))
    todo!()
}
