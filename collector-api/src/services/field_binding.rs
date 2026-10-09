use std::collections::HashMap;

use collector_core::{
    core::point::PointRef,
    field::{FieldDirection, field_registry},
    utils::database::get_database,
};
use serde::Serialize;
use sqlx::SqlitePool;

use crate::{
    dao::field_binding::{FieldBindingDao, UpsertFieldBinding},
    handlers::field_binding::SetFieldBindingParams,
    models::field_binding::PointKind,
    services::{ServiceError, ServiceResult},
};

#[derive(Debug, Serialize)]
pub struct FieldBindingView {
    pub field_key: String,
    pub name: String,
    pub direction: String,
    pub dev_id: String,
    pub point_kind: PointKind,
    pub point_value: String,
    pub is_override: bool,
    pub updated_by: Option<String>,
}

fn direction_str(direction: FieldDirection) -> &'static str {
    match direction {
        FieldDirection::Read => "read",
        FieldDirection::Write => "write",
        FieldDirection::ReadWrite => "read_write",
    }
}

fn default_point_kind(point: &PointRef) -> PointKind {
    match point {
        PointRef::Id(_) => PointKind::Id,
        PointRef::Key(_) => PointKind::Key,
        PointRef::Name(_) => PointKind::Name,
    }
}

fn default_point_value(point: &PointRef) -> String {
    match point {
        PointRef::Id(id) => id.to_string(),
        PointRef::Key(k) => k.clone(),
        PointRef::Name(n) => n.clone(),
    }
}

pub struct FieldBindingService {
    pool: SqlitePool,
}

impl FieldBindingService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            pool: get_database()?,
        })
    }

    /// 合并已注册字段的默认值与覆盖表，返回每个字段当前生效的绑定视图
    pub async fn list(&self) -> ServiceResult<Vec<FieldBindingView>> {
        let overrides = FieldBindingDao::find_all_enabled(&self.pool).await?;
        let overrides_by_key: HashMap<_, _> = overrides
            .into_iter()
            .map(|o| (o.field_key.clone(), o))
            .collect();

        let mut specs = field_registry().all_specs();
        specs.sort_by_key(|s| s.key);

        let views = specs
            .into_iter()
            .map(|spec| match overrides_by_key.get(spec.key) {
                Some(o) => FieldBindingView {
                    field_key: spec.key.to_string(),
                    name: spec.name.to_string(),
                    direction: direction_str(spec.direction).to_string(),
                    dev_id: o.dev_id.clone(),
                    point_kind: o.point_kind,
                    point_value: o.point_value.clone(),
                    is_override: true,
                    updated_by: o.updated_by.clone(),
                },
                None => FieldBindingView {
                    field_key: spec.key.to_string(),
                    name: spec.name.to_string(),
                    direction: direction_str(spec.direction).to_string(),
                    dev_id: spec.default_dev.to_string(),
                    point_kind: default_point_kind(&spec.default_point),
                    point_value: default_point_value(&spec.default_point),
                    is_override: false,
                    updated_by: None,
                },
            })
            .collect();
        Ok(views)
    }

    pub async fn set_binding(
        &self,
        params: SetFieldBindingParams,
        updated_by: Option<String>,
    ) -> ServiceResult<()> {
        let spec = field_registry()
            .spec(&params.field_key)
            .ok_or_else(|| ServiceError::NotFound(format!("字段`{}`未注册", params.field_key)))?;
        // DataCenter 本身不支持按 Name 读取，可读字段绑定为 Name 会导致读取永远返回空
        if params.point_kind == PointKind::Name
            && matches!(
                spec.direction,
                FieldDirection::Read | FieldDirection::ReadWrite
            )
        {
            return Err(ServiceError::InvalidParameter(
                "该字段可读，不支持绑定为 Name 引用".to_string(),
            ));
        }
        if params.point_kind == PointKind::Id && params.point_value.parse::<u32>().is_err() {
            return Err(ServiceError::InvalidParameter(
                "point_kind为Id时point_value必须是合法的点位ID".to_string(),
            ));
        }
        FieldBindingDao::upsert(
            &self.pool,
            UpsertFieldBinding {
                field_key: &params.field_key,
                dev_id: &params.dev_id,
                point_kind: params.point_kind,
                point_value: &params.point_value,
                updated_by: updated_by.as_deref(),
            },
        )
        .await?;
        field_registry()
            .reload(&self.pool)
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        Ok(())
    }

    pub async fn reset_binding(
        &self,
        field_key: &str,
        updated_by: Option<String>,
    ) -> ServiceResult<()> {
        let rows = FieldBindingDao::reset(&self.pool, field_key, updated_by.as_deref()).await?;
        if rows == 0 {
            return Err(ServiceError::NotFound(format!(
                "字段`{field_key}`没有生效中的覆盖绑定"
            )));
        }
        field_registry()
            .reload(&self.pool)
            .await
            .map_err(|e| ServiceError::InternalError(e.to_string()))?;
        Ok(())
    }
}
