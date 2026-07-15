use collector_core::down;
use salvo::Depot;

use crate::{
    handlers::data::RequestDataParams,
    services::{Service, ServiceError, ServiceResult},
};

pub struct DataService {}

impl Service for DataService {}

impl DataService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {})
    }

    pub async fn set(&self, depot: &mut Depot, params: RequestDataParams) -> ServiceResult<()> {
        if params.points.is_empty() {
            return Err(ServiceError::InvalidParameter(String::from(
                "points不能为空",
            )));
        }
        let center = self.center(depot)?;
        let ids = center.dev_ids();
        let json = serde_json::to_string(&params)
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        tracing::info!("set points: {}", json);
        for param in params.points {
            if !ids.contains(&param.dev_id) && !center.has_downlink(&param.dev_id) {
                return Err(ServiceError::InvalidParameter(format!(
                    "设备ID {} 不存在",
                    param.dev_id
                )));
            }
            if let Some(id) = param.point_id {
                let point = down!(id: id, param.value);
                center
                    .dispatch(&param.dev_id, vec![point])
                    .await
                    .map_err(|e| ServiceError::InternalError(e.to_string()))?;
            } else if let Some(key) = param.point_key {
                let point = down!(key: key, param.value);
                center
                    .dispatch(&param.dev_id, vec![point])
                    .await
                    .map_err(|e| ServiceError::InternalError(e.to_string()))?;
            } else {
                return Err(ServiceError::InvalidParameter(
                    "point_id和point_key不能同时为空".to_string(),
                ));
            }
        }
        Ok(())
    }
}
