use salvo::{Request, handler};
use serde::Deserialize;
use validator::Validate;

use crate::{
    core::{ApiResult, response::ObjResponse},
    services::arbitrage::{ArbitragePlan, ArbitrageService},
};

fn default_efficiency() -> f64 {
    0.9
}

fn default_soc_step() -> f64 {
    1.0
}

/// 峰谷套利曲线生成参数，每次请求携带，不落库
#[derive(Debug, Clone, Deserialize, Validate)]
pub struct ArbitrageParams {
    /// 最大充电功率(kW)
    #[validate(range(
        exclusive_min = 0.0,
        max = 100_000.0,
        message = "最大充电功率必须大于0"
    ))]
    pub max_charge_kw: f64,
    /// 最大放电功率(kW)
    #[validate(range(
        exclusive_min = 0.0,
        max = 100_000.0,
        message = "最大放电功率必须大于0"
    ))]
    pub max_discharge_kw: f64,
    /// 电池容量(kWh)
    #[validate(range(exclusive_min = 0.0, max = 1_000_000.0, message = "电池容量必须大于0"))]
    pub capacity_kwh: f64,
    /// SOC 下限(%)，放电不低于此值
    #[validate(range(min = 0.0, max = 100.0, message = "soc_min必须在0-100之间"))]
    pub soc_min: f64,
    /// SOC 上限(%)，充电不高于此值
    #[validate(range(min = 0.0, max = 100.0, message = "soc_max必须在0-100之间"))]
    pub soc_max: f64,
    /// 当天开始时的 SOC(%)
    #[validate(range(min = 0.0, max = 100.0, message = "initial_soc必须在0-100之间"))]
    pub initial_soc: f64,
    /// 当天结束时的目标 SOC(%)，默认与 initial_soc 相同，使曲线每天重复执行不累积漂移
    #[validate(range(min = 0.0, max = 100.0, message = "end_soc必须在0-100之间"))]
    pub end_soc: Option<f64>,
    /// 往返效率(0.5-1.0)，默认0.9，充放电各按其平方根计
    #[serde(default = "default_efficiency")]
    #[validate(range(min = 0.5, max = 1.0, message = "往返效率必须在0.5-1.0之间"))]
    pub round_trip_efficiency: f64,
    /// 度电衰减成本(元/kWh)，按充放电电量计，默认0
    #[serde(default)]
    #[validate(range(min = 0.0, max = 1000.0, message = "度电衰减成本必须在0-1000之间"))]
    pub degradation_cost_per_kwh: f64,
    /// SOC 离散步长(%)，越小越精确、计算越慢，默认1
    #[serde(default = "default_soc_step")]
    #[validate(range(min = 0.1, max = 5.0, message = "soc_step必须在0.1-5之间"))]
    pub soc_step: f64,
}

#[derive(Debug, Deserialize, Validate)]
pub struct ApplyArbitrageParams {
    /// 写入的计划曲线 ID，其 96 个时段明细会被整体覆盖
    pub curve_id: u32,
    #[serde(flatten)]
    #[validate(nested)]
    pub params: ArbitrageParams,
}

#[handler]
pub async fn preview(req: &mut Request) -> ApiResult<ObjResponse<ArbitragePlan>> {
    let params = req.parse_json::<ArbitrageParams>().await?;
    params.validate()?;
    let service = ArbitrageService::new()?;
    Ok(ObjResponse::ok(service.plan(params).await?))
}

#[handler]
pub async fn apply(req: &mut Request) -> ApiResult<ObjResponse<ArbitragePlan>> {
    let params = req.parse_json::<ApplyArbitrageParams>().await?;
    params.validate()?;
    let service = ArbitrageService::new()?;
    Ok(ObjResponse::ok(
        service.apply(params.curve_id, params.params).await?,
    ))
}
