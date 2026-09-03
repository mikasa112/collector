use sqlx::{QueryBuilder, Sqlite, SqlitePool};

use crate::{dao::error::DaoResult, models::alarm::Alarm};

pub struct AlarmDao;

/// 告警列表可排序的字段
pub enum AlarmSortField {
    CreatedAt,
    AlarmLevel,
}

impl AlarmSortField {
    fn column(&self) -> &'static str {
        match self {
            AlarmSortField::CreatedAt => "created_at",
            AlarmSortField::AlarmLevel => "alarm_level",
        }
    }
}

pub enum SortOrder {
    Asc,
    Desc,
}

impl SortOrder {
    fn sql(&self) -> &'static str {
        match self {
            SortOrder::Asc => "ASC",
            SortOrder::Desc => "DESC",
        }
    }
}

/// 告警历史查询条件
pub struct AlarmListQuery<'a> {
    /// 设备类型，如 "pcs"/"bcu"/"tms"
    pub alarm_dev: Option<&'a str>,
    /// 告警名称，模糊匹配
    pub alarm_name: Option<&'a str>,
    pub sort_field: AlarmSortField,
    pub sort_order: SortOrder,
    pub limit: u32,
    pub offset: u32,
}

fn push_filters<'a>(
    qb: &mut QueryBuilder<'a, Sqlite>,
    alarm_dev: Option<&'a str>,
    alarm_name: Option<&'a str>,
) {
    if let Some(dev) = alarm_dev {
        qb.push(" AND alarm_dev = ").push_bind(dev);
    }
    if let Some(name) = alarm_name {
        qb.push(" AND alarm_name LIKE ")
            .push_bind(format!("%{name}%"));
    }
}

impl AlarmDao {
    /// 分页查询告警历史，正在发生的（Active）始终排在前面，
    /// 组内再按 query 指定的字段/方向排序
    pub async fn find_all(pool: &SqlitePool, query: &AlarmListQuery<'_>) -> DaoResult<Vec<Alarm>> {
        let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
            "SELECT id, alarm_code, alarm_name, alarm_dev, alarm_level, alarm_status, created_by, created_at, updated_at
             FROM t_alarm
             WHERE deleted_at IS NULL",
        );
        push_filters(&mut qb, query.alarm_dev, query.alarm_name);
        qb.push(" ORDER BY alarm_status ASC, ")
            .push(query.sort_field.column())
            .push(" ")
            .push(query.sort_order.sql())
            .push(" LIMIT ")
            .push_bind(query.limit)
            .push(" OFFSET ")
            .push_bind(query.offset);
        let alarms = qb.build_query_as::<Alarm>().fetch_all(pool).await?;
        Ok(alarms)
    }

    pub async fn count_all(pool: &SqlitePool, query: &AlarmListQuery<'_>) -> DaoResult<usize> {
        let mut qb: QueryBuilder<Sqlite> =
            QueryBuilder::new("SELECT COUNT(*) FROM t_alarm WHERE deleted_at IS NULL");
        push_filters(&mut qb, query.alarm_dev, query.alarm_name);
        let total: i64 = qb.build_query_scalar().fetch_one(pool).await?;
        Ok(total as usize)
    }
}
