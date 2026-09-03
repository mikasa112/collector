use chrono::NaiveDateTime;
use serde::{Serialize, Serializer};
use sqlx::prelude::{FromRow, Type};

#[derive(Debug, Type, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum AlarmLevel {
    Minor = 1,
    Major = 2,
    Critical = 3,
}

impl Serialize for AlarmLevel {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.clone() as u8)
    }
}

#[derive(Debug, Type, Clone, PartialEq, Eq)]
#[repr(u8)]
pub enum AlarmStatus {
    Active = 0,
    Recovered = 1,
}

impl Serialize for AlarmStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(self.clone() as u8)
    }
}

#[derive(FromRow, Debug, Serialize)]
pub struct Alarm {
    pub id: u32,
    pub alarm_code: u32,
    pub alarm_name: String,
    pub alarm_dev: String,
    pub alarm_level: AlarmLevel,
    pub alarm_status: AlarmStatus,
    pub created_by: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}
