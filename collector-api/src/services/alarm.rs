use collector_core::utils::database::get_database;
use sqlx::SqlitePool;

use crate::{
    dao::alarm::{AlarmDao, AlarmListQuery, AlarmSortField, SortOrder},
    handlers::alarm::AlarmListParams,
    models::alarm::Alarm,
    services::{ServiceError, ServiceResult},
};

/// 支持按设备类型过滤的白名单
const ALLOWED_DEVS: &[&str] = &["pcs", "bcu", "tms"];

pub struct AlarmService {
    pool: SqlitePool,
}

impl AlarmService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            pool: get_database()?,
        })
    }

    pub async fn list(&self, params: &AlarmListParams) -> ServiceResult<(Vec<Alarm>, usize)> {
        let page = params.page.unwrap_or(1).max(1);
        let size = params.size.unwrap_or(10).clamp(1, 100);

        let alarm_dev = match params.alarm_dev.as_deref() {
            Some(dev) if ALLOWED_DEVS.contains(&dev) => Some(dev),
            Some(dev) => {
                return Err(ServiceError::InvalidParameter(format!(
                    "不支持的设备类型: {dev}"
                )));
            }
            None => None,
        };
        let sort_field = match params.sort_by.as_deref() {
            None | Some("created_at") => AlarmSortField::CreatedAt,
            Some("alarm_level") => AlarmSortField::AlarmLevel,
            Some(other) => {
                return Err(ServiceError::InvalidParameter(format!(
                    "不支持的排序字段: {other}"
                )));
            }
        };
        let sort_order = match params.order.as_deref() {
            None | Some("desc") => SortOrder::Desc,
            Some("asc") => SortOrder::Asc,
            Some(other) => {
                return Err(ServiceError::InvalidParameter(format!(
                    "不支持的排序方向: {other}"
                )));
            }
        };

        let query = AlarmListQuery {
            alarm_dev,
            alarm_name: params.alarm_name.as_deref(),
            sort_field,
            sort_order,
            limit: size,
            offset: (page - 1) * size,
        };
        let list = AlarmDao::find_all(&self.pool, &query).await?;
        let total = AlarmDao::count_all(&self.pool, &query).await?;
        Ok((list, total))
    }
}
