use argon2::{
    Argon2, PasswordVerifier,
    password_hash::{PasswordHash, PasswordHasher, SaltString, rand_core::OsRng},
};
use collector_core::utils::database::get_database;
use jsonwebtoken::{EncodingKey, Header};
use salvo::http::cookie::time::{Duration, OffsetDateTime};
use sqlx::SqlitePool;

use crate::{
    dao::user::UserDao,
    handlers::user::{CreateUserParams, LoginParams},
    middleware::auth::{JWT_SECRET, JwtClaims},
    models::user::Role,
    services::{ServiceError, ServiceResult},
};

pub struct UserService {
    pool: SqlitePool,
}

impl UserService {
    pub fn new() -> ServiceResult<Self> {
        Ok(Self {
            pool: get_database()?,
        })
    }

    pub async fn login(&self, params: LoginParams) -> ServiceResult<String> {
        let user = UserDao::find_by_account(&self.pool, &params.username)
            .await?
            .ok_or_else(|| ServiceError::auth_failed("用户不存在"))?;

        // 在后台线程中验证密码（CPU 密集型操作）
        let password = params.password.clone();
        let stored_hash = user.password.clone();

        let argon2_result =
            tokio::task::spawn_blocking(move || match PasswordHash::new(&stored_hash) {
                Ok(parsed_hash) => {
                    match Argon2::default().verify_password(password.as_bytes(), &parsed_hash) {
                        Ok(_) => Ok(()),
                        Err(_) => Err(ServiceError::auth_failed("密码错误")),
                    }
                }
                Err(_) => Err(ServiceError::auth_failed("密码错误")),
            })
            .await?;

        argon2_result?;

        // 生成 JWT Token，有效期为 1 天
        let exp = OffsetDateTime::now_utc() + Duration::days(1);
        let claims = JwtClaims {
            username: user.account,
            role: user.role.as_str().to_string(),
            exp: exp.unix_timestamp(),
        };

        let token = jsonwebtoken::encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(JWT_SECRET),
        )
        .map_err(|e| ServiceError::auth_failed(e.to_string()))?;

        Ok(token)
    }

    pub async fn create_user(&self, params: CreateUserParams) -> ServiceResult<()> {
        // 检查用户是否已存在
        let user = UserDao::find_by_account(&self.pool, &params.username).await?;
        if user.is_some() {
            return Err(ServiceError::already_exists("账号已存在"));
        }
        // 解析角色
        let role = Role::try_from(params.role.as_str())
            .map_err(|e| ServiceError::invalid_parameter(e.to_string()))?;

        // 验证密码强度
        if params.password.len() < 6 {
            return Err(ServiceError::invalid_parameter("密码长度不能少于 6 位"));
        }

        let password = params.password.clone();
        let password_hash = tokio::task::spawn_blocking(move || {
            let salt = SaltString::generate(&mut OsRng);
            let argon2 = Argon2::default();
            argon2
                .hash_password(password.as_bytes(), &salt)
                .map(|hash| hash.to_string())
                .map_err(|e| ServiceError::business_logic(format!("密码加密失败: {}", e)))
        })
        .await??;

        // 创建用户
        UserDao::create(
            &self.pool,
            &params.username,
            &password_hash,
            params.name.as_deref(),
            role,
        )
        .await?;

        Ok(())
    }
}
