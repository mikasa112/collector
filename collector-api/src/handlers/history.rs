use salvo::{Request, handler};

use crate::{
    core::{ApiResult, response::ObjResponse},
    services::history::{FieldHistory, HistoryQueryParams, HistoryService},
};

#[handler]
pub async fn pcs_history(req: &mut Request) -> ApiResult<ObjResponse<Vec<FieldHistory>>> {
    let params = req.parse_queries::<HistoryQueryParams>()?;
    let service = HistoryService::new()?;
    let data = service.pcs_history(params).await?;
    Ok(ObjResponse::ok(data))
}

#[handler]
pub async fn bcu_history(req: &mut Request) -> ApiResult<ObjResponse<Vec<FieldHistory>>> {
    let params = req.parse_queries::<HistoryQueryParams>()?;
    let service = HistoryService::new()?;
    let data = service.bcu_history(params).await?;
    Ok(ObjResponse::ok(data))
}
