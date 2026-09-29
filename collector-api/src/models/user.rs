use chrono::NaiveDateTime;
use serde::Serialize;
use sqlx::prelude::{FromRow, Type};

/// 用户角色，按权限从低到高声明，声明顺序即 derive(Ord) 的比较顺序，
/// 可直接用 `role >= Role::Admin` 做层级校验。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Type, Serialize)]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Role {
    User,
    Admin,
    SuperAdmin,
}

impl TryFrom<&str> for Role {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "super_admin" => Ok(Role::SuperAdmin),
            "admin" => Ok(Role::Admin),
            "user" => Ok(Role::User),
            _ => Err(anyhow::anyhow!("Invalid role: {}", value)),
        }
    }
}

impl Role {
    pub fn as_str(&self) -> &str {
        match self {
            Role::SuperAdmin => "super_admin",
            Role::Admin => "admin",
            Role::User => "user",
        }
    }
}

#[derive(FromRow, Debug)]
#[allow(dead_code)]
pub struct User {
    pub id: u32,
    pub name: Option<String>,
    pub account: String,
    pub password: String,
    pub role: Role,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub deleted_at: Option<NaiveDateTime>,
}

/// 用户信息的对外展示视图，不包含密码哈希
#[derive(Debug, Serialize)]
pub struct UserSummary {
    pub id: u32,
    pub name: Option<String>,
    pub account: String,
    pub role: Role,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

impl From<User> for UserSummary {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            name: user.name,
            account: user.account,
            role: user.role,
            created_at: user.created_at,
            updated_at: user.updated_at,
        }
    }
}
