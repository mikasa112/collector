use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::prelude::{FromRow, Type};

/// 点位引用方式，对应 collector_core::core::point::PointRef 的三种变体
#[derive(Debug, Type, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PointKind {
    Id = 1,
    Key = 2,
    Name = 3,
}

#[derive(FromRow, Debug, Serialize)]
pub struct FieldBindingOverride {
    pub id: u32,
    pub field_key: String,
    pub dev_id: String,
    pub point_kind: PointKind,
    pub point_value: String,
    pub enabled: bool,
    pub updated_by: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}
