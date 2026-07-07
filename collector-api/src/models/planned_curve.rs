use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use sqlx::prelude::{FromRow, Type};

#[derive(Debug, Type, Serialize, Deserialize, Clone)]
#[repr(u8)]
pub enum CurveType {
    Day = 1,
    Week = 2,
    Custom = 3,
}

#[derive(FromRow, Debug, Serialize)]
pub struct PlanCurveMaster {
    pub id: u32,
    pub curve_name: String,
    pub curve_type: CurveType,
    //优先级，数字越小优先级越高
    pub priority: Option<u8>,
    //状态：0-草稿 1-已发布 2-执行中 3-已归档
    pub status: Option<u8>,
    //生效起始时间
    pub valid_start_date: Option<String>,
    //生效结束时间
    pub valid_end_date: Option<String>,
    //生效星期掩码，如 "1,2,3,4,5" 表示周一至周五
    pub effective_weekdays: Option<String>,
    pub created_by: Option<String>,
    pub remark: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub deleted_at: Option<NaiveDateTime>,
}

#[derive(FromRow, Debug, Serialize)]
pub struct PlanCurveDetail {
    pub id: u32,
    pub curve_id: u32,
    // 0-95，对应00:00-23:45, 时间粒度15分钟
    pub time_index: u8,
    //功率
    pub power_value: f64,
    //SOC限制
    pub soc_limit: Option<f64>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub deleted_at: Option<NaiveDateTime>,
}
