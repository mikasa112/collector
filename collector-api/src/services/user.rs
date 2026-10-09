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
    handlers::user::{CreateUserParams, LoginParams, UpdateUserParams},
    middleware::auth::{JwtClaims, jwt_secret},
    models::user::{Role, UserSummary},
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
            &EncodingKey::from_secret(jwt_secret()),
        )
        .map_err(|e| ServiceError::auth_failed(e.to_string()))?;

        Ok(token)
    }

    /// 校验调用者是否有权把某账号设为 target_role
    ///
    /// 只有超级管理员能分配 Admin/SuperAdmin 角色；管理员只能分配 User 角色。
    fn ensure_can_assign_role(caller_role: Role, target_role: Role) -> ServiceResult<()> {
        match caller_role {
            Role::SuperAdmin => Ok(()),
            Role::Admin if target_role == Role::User => Ok(()),
            _ => Err(ServiceError::permission_denied("无权将账号设置为该角色")),
        }
    }

    /// 校验调用者是否有权操作(改密/改角色/删除)某个已存在账号
    ///
    /// 管理员不能操作角色等级不低于自己的账号（即其他 Admin/SuperAdmin），
    /// 超级管理员不受限制。
    fn ensure_can_manage_target(caller_role: Role, target_current_role: Role) -> ServiceResult<()> {
        if caller_role == Role::SuperAdmin {
            return Ok(());
        }
        if target_current_role >= Role::Admin {
            return Err(ServiceError::permission_denied("无权操作该账号"));
        }
        Ok(())
    }

    pub async fn create_user(
        &self,
        caller_role: Role,
        params: CreateUserParams,
    ) -> ServiceResult<()> {
        // 检查用户是否已存在
        let user = UserDao::find_by_account(&self.pool, &params.username).await?;
        if user.is_some() {
            return Err(ServiceError::already_exists("账号已存在"));
        }
        // 解析角色
        let role = Role::try_from(params.role.as_str())
            .map_err(|e| ServiceError::invalid_parameter(e.to_string()))?;

        Self::ensure_can_assign_role(caller_role, role)?;

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

    pub async fn list_users(&self) -> ServiceResult<Vec<UserSummary>> {
        let users = UserDao::find_all(&self.pool).await?;
        Ok(users.into_iter().map(UserSummary::from).collect())
    }

    pub async fn update_user(
        &self,
        caller_role: Role,
        id: u32,
        params: UpdateUserParams,
    ) -> ServiceResult<()> {
        let target = UserDao::find_by_id(&self.pool, id)
            .await?
            .ok_or_else(|| ServiceError::not_found("用户不存在"))?;

        Self::ensure_can_manage_target(caller_role, target.role)?;

        let new_role = match &params.role {
            Some(role_str) => {
                let role = Role::try_from(role_str.as_str())
                    .map_err(|e| ServiceError::invalid_parameter(e.to_string()))?;
                Self::ensure_can_assign_role(caller_role, role)?;
                // 防止把系统中最后一个超级管理员降级
                if target.role == Role::SuperAdmin
                    && role != Role::SuperAdmin
                    && UserDao::count_by_role(&self.pool, Role::SuperAdmin).await? <= 1
                {
                    return Err(ServiceError::permission_denied(
                        "不能降级系统中唯一的超级管理员",
                    ));
                }
                Some(role)
            }
            None => None,
        };

        if let Some(password) = &params.password {
            if password.len() < 6 {
                return Err(ServiceError::invalid_parameter("密码长度不能少于 6 位"));
            }
        }

        let password_hash = match params.password.clone() {
            Some(password) => Some(
                tokio::task::spawn_blocking(move || {
                    let salt = SaltString::generate(&mut OsRng);
                    let argon2 = Argon2::default();
                    argon2
                        .hash_password(password.as_bytes(), &salt)
                        .map(|hash| hash.to_string())
                        .map_err(|e| ServiceError::business_logic(format!("密码加密失败: {}", e)))
                })
                .await??,
            ),
            None => None,
        };

        UserDao::update(
            &self.pool,
            id,
            params.name.as_deref(),
            password_hash.as_deref(),
            new_role,
        )
        .await?;

        Ok(())
    }

    pub async fn delete_user(&self, caller_role: Role, id: u32) -> ServiceResult<()> {
        let target = UserDao::find_by_id(&self.pool, id)
            .await?
            .ok_or_else(|| ServiceError::not_found("用户不存在"))?;

        Self::ensure_can_manage_target(caller_role, target.role)?;

        if target.role == Role::SuperAdmin
            && UserDao::count_by_role(&self.pool, Role::SuperAdmin).await? <= 1
        {
            return Err(ServiceError::permission_denied(
                "不能删除系统中唯一的超级管理员",
            ));
        }

        UserDao::soft_delete(&self.pool, id).await?;

        Ok(())
    }
}
