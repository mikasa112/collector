use salvo::{Request, handler};
use serde::Deserialize;

use crate::{
    core::{ApiResult, response::ListResponse},
    models::alarm::Alarm,
    services::alarm::AlarmService,
};

#[derive(Debug, Deserialize)]
pub struct AlarmListParams {
    pub page: Option<u32>,
    pub size: Option<u32>,
    /// 设备类型过滤，如 "pcs"/"bcu"/"tms"
    pub alarm_dev: Option<String>,
    /// 告警名称，模糊匹配
    pub alarm_name: Option<String>,
    /// 排序字段："created_at"（默认）或 "alarm_level"
    pub sort_by: Option<String>,
    /// 排序方向："asc" 或 "desc"（默认）
    pub order: Option<String>,
}

/// 分页查询告警历史，正在发生的（未恢复）告警始终排在前面
#[handler]
pub async fn list(req: &mut Request) -> ApiResult<ListResponse<Alarm>> {
    let params = req.parse_queries::<AlarmListParams>()?;
    let service = AlarmService::new()?;
    let (alarms, total) = service.list(&params).await?;
    Ok(ListResponse::ok(alarms, total))
}
