use serde::{Deserialize, Serialize};
use sqlx::prelude::{FromRow, Type};

/// 电价时段类型：尖、峰、平、谷
#[derive(Debug, Type, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum PeriodType {
    Sharp = 1,
    Peak = 2,
    Flat = 3,
    Valley = 4,
}

impl PeriodType {
    pub const ALL: [PeriodType; 4] = [Self::Sharp, Self::Peak, Self::Flat, Self::Valley];

    pub fn name(self) -> &'static str {
        match self {
            Self::Sharp => "尖",
            Self::Peak => "峰",
            Self::Flat => "平",
            Self::Valley => "谷",
        }
    }
}

#[derive(FromRow, Debug)]
pub struct ElectricityPrice {
    pub period_type: PeriodType,
    pub price: f64,
}

#[derive(FromRow, Debug, Serialize)]
pub struct ElectricityPeriod {
    pub start_time: String,
    pub end_time: String,
    pub period_type: PeriodType,
}
