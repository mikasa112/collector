use salvo::{Depot, Request, handler};
use validator::Validate;

use crate::{
    core::{
        ApiResult,
        response::{ListResponse, ObjResponse},
    },
    middleware::auth::current_username,
    models::electricity::{ElectricityPeriod, PeriodType},
    services::electricity::{ElectricityService, PriceView},
};

/// 各类型单价(元/kWh)，只传需要修改的类型
#[derive(Debug, serde::Deserialize, Validate)]
pub struct UpdatePricesParams {
    #[validate(range(min = 0.0, max = 1000.0, message = "尖单价必须在0-1000之间"))]
    pub sharp: Option<f64>,
    #[validate(range(min = 0.0, max = 1000.0, message = "峰单价必须在0-1000之间"))]
    pub peak: Option<f64>,
    #[validate(range(min = 0.0, max = 1000.0, message = "平单价必须在0-1000之间"))]
    pub flat: Option<f64>,
    #[validate(range(min = 0.0, max = 1000.0, message = "谷单价必须在0-1000之间"))]
    pub valley: Option<f64>,
}

#[derive(Debug, serde::Deserialize)]
pub struct PeriodParams {
    pub start_time: String,
    pub end_time: String,
    pub period_type: PeriodType,
}

#[derive(Debug, serde::Deserialize)]
pub struct ReplacePeriodsParams {
    pub periods: Vec<PeriodParams>,
}

#[handler]
pub async fn list_prices(_req: &mut Request) -> ApiResult<ListResponse<PriceView>> {
    let service = ElectricityService::new()?;
    let result = service.prices().await?;
    let len = result.len();
    Ok(ListResponse::ok(result, len))
}

#[handler]
pub async fn update_prices(req: &mut Request, depot: &mut Depot) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<UpdatePricesParams>().await?;
    params.validate()?;
    let service = ElectricityService::new()?;
    service
        .update_prices(params, current_username(depot))
        .await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn list_periods(_req: &mut Request) -> ApiResult<ListResponse<ElectricityPeriod>> {
    let service = ElectricityService::new()?;
    let result = service.periods().await?;
    let len = result.len();
    Ok(ListResponse::ok(result, len))
}

#[handler]
pub async fn replace_periods(req: &mut Request, depot: &mut Depot) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<ReplacePeriodsParams>().await?;
    let service = ElectricityService::new()?;
    service
        .replace_periods(params.periods, current_username(depot))
        .await?;
    Ok(ObjResponse::ok(()))
}
