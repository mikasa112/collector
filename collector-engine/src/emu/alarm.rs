use collector_core::core::point::WarnLevel;
use sqlx::{SqlitePool, prelude::Type};

#[repr(u8)]
#[derive(Debug, Type, Clone)]
pub(crate) enum AlaramLevel {
    Minor = 1,
    Major = 2,
    Critical = 3,
}

#[repr(u8)]
#[derive(Debug, Type, Clone)]
pub(crate) enum AlaramStatus {
    Active = 0,
    Recovered = 1,
}

impl From<WarnLevel> for AlaramLevel {
    fn from(level: WarnLevel) -> Self {
        match level {
            WarnLevel::Critical => AlaramLevel::Critical,
            WarnLevel::High => AlaramLevel::Major,
            WarnLevel::Normal | WarnLevel::None => AlaramLevel::Minor,
        }
    }
}

pub(crate) struct AlarmDao {
    pub(crate) pool: SqlitePool,
}

impl AlarmDao {
    pub(crate) async fn create_alarm(
        &self,
        alarm_code: u32,
        alarm_name: String,
        alarm_dev: String,
        alarm_level: AlaramLevel,
        alaram_status: AlaramStatus,
        created_by: String,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO t_alarm (alarm_code, alarm_name, alarm_dev, alarm_level, alarm_status, created_by)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(alarm_code)
        .bind(alarm_name)
        .bind(alarm_dev)
        .bind(alarm_level)
        .bind(alaram_status)
        .bind(created_by)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub(crate) async fn update_alarm_status(
        &self,
        alarm_code: u32,
        alarm_dev: String,
        alarm_satus: AlaramStatus,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE t_alarm
             SET alarm_status = ?, updated_at = datetime('now', 'localtime')
             WHERE alarm_code = ?
               AND alarm_dev = ?
               AND deleted_at IS NULL",
        )
        .bind(alarm_satus)
        .bind(alarm_code)
        .bind(alarm_dev)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// 查询当前仍处于 Active 状态的告警的 (alarm_code, alarm_dev)，
    /// 用于与本次 tick 计算出的告警集合做差集
    pub(crate) async fn list_active_keys(&self) -> Result<Vec<(u32, String)>, sqlx::Error> {
        sqlx::query_as(
            "SELECT alarm_code, alarm_dev
             FROM t_alarm
             WHERE alarm_status = ?
               AND deleted_at IS NULL",
        )
        .bind(AlaramStatus::Active)
        .fetch_all(&self.pool)
        .await
    }
}
