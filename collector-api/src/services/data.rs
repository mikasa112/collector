use collector_core::center::data_center;
use collector_core::down;
use collector_core::runtime::{core::get_runtime, emu::is_run_mode_point};

use crate::{
    handlers::data::{RequestDataParam, RequestDataParams},
    services::{ServiceError, ServiceResult},
};

pub struct DataService {}

impl DataService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {})
    }

    pub async fn set(&self, params: RequestDataParams) -> ServiceResult<()> {
        if params.points.is_empty() {
            return Err(ServiceError::InvalidParameter(String::from(
                "points不能为空",
            )));
        }
        // EMU控制源为远程时由北向控制，不允许通过API修改点位；
        // 控制源点位本身除外，否则远程模式下无法切回本地
        let runtime = get_runtime()
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        if !runtime.emu_runtime.allow_api_dispatch()
            && params
                .points
                .iter()
                .any(|p| !is_control_source(p) && !is_run_mode(p))
        {
            return Err(ServiceError::PermissionDenied(String::from(
                "EMU控制源为远程，不允许通过API修改点位",
            )));
        }
        let center = data_center();
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

/// EMU运行模式点位（emu 设备 8 号点位），不受控制源限制
fn is_run_mode(p: &RequestDataParam) -> bool {
    is_run_mode_point(&p.dev_id, p.point_id, p.point_key.as_deref())
}

/// EMU控制源点位（emu 设备 9 号点位）
fn is_control_source(p: &RequestDataParam) -> bool {
    p.dev_id == "emu" && (p.point_id == Some(9) || p.point_key.as_deref() == Some("control_source"))
}
