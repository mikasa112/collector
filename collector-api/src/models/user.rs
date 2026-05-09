use chrono::NaiveDateTime;
use sqlx::prelude::{FromRow, Type};

#[derive(Debug, Type)]
#[sqlx(type_name = "TEXT", rename_all = "lowercase")]
pub enum Role {
    Admin,
    User,
    Guest,
}

impl TryFrom<&str> for Role {
    type Error = anyhow::Error;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "admin" => Ok(Role::Admin),
            "user" => Ok(Role::User),
            "guest" => Ok(Role::Guest),
            _ => Err(anyhow::anyhow!("Invalid role: {}", value)),
        }
    }
}

impl Role {
    pub fn as_str(&self) -> &str {
        match self {
            Role::Admin => "admin",
            Role::User => "user",
            Role::Guest => "guest",
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
