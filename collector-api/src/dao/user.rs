use crate::{
    dao::error::{DaoError, DaoResult},
    models::user::{Role, User},
};
use sqlx::SqlitePool;

/// 用户数据访问对象
pub struct UserDao;

impl UserDao {
    /// 创建用户
    pub async fn create(
        pool: &SqlitePool,
        account: &str,
        password: &str,
        name: Option<&str>,
        role: Role,
    ) -> DaoResult<i64> {
        // 检查账号是否已存在
        if Self::exists_by_account(pool, account).await? {
            return Err(DaoError::AlreadyExists(format!(
                "账号 '{}' 已存在",
                account
            )));
        }

        let result = sqlx::query(
            "INSERT INTO t_user (account, password, name, role, created_at, updated_at) VALUES (?, ?, ?, ?, datetime('now'), datetime('now'))"
        )
        .bind(account)
        .bind(password)
        .bind(name)
        .bind(role)
        .execute(pool)
        .await?;

        Ok(result.last_insert_rowid())
    }

    /// 根据 ID 查询用户
    pub async fn find_by_id(pool: &SqlitePool, id: u32) -> DaoResult<Option<User>> {
        let user = sqlx::query_as::<_, User>(
            "SELECT id, name, account, password, role, created_at, updated_at, deleted_at FROM t_user WHERE id = ? AND deleted_at IS NULL"
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;

        Ok(user)
    }

    /// 根据账号查询用户
    pub async fn find_by_account(pool: &SqlitePool, account: &str) -> DaoResult<Option<User>> {
        let user = sqlx::query_as::<_, User>(
            "SELECT id, name, account, password, role, created_at, updated_at, deleted_at FROM t_user WHERE account = ? AND deleted_at IS NULL"
        )
        .bind(account)
        .fetch_optional(pool)
        .await?;

        Ok(user)
    }

    /// 查询所有用户
    pub async fn find_all(pool: &SqlitePool) -> DaoResult<Vec<User>> {
        let users = sqlx::query_as::<_, User>(
            "SELECT id, name, account, password, role, created_at, updated_at, deleted_at FROM t_user WHERE deleted_at IS NULL ORDER BY id"
        )
        .fetch_all(pool)
        .await?;

        Ok(users)
    }

    /// 根据角色查询用户
    pub async fn find_by_role(pool: &SqlitePool, role: Role) -> DaoResult<Vec<User>> {
        let users = sqlx::query_as::<_, User>(
            "SELECT id, name, account, password, role, created_at, updated_at, deleted_at FROM t_user WHERE role = ? AND deleted_at IS NULL ORDER BY id"
        )
        .bind(role)
        .fetch_all(pool)
        .await?;

        Ok(users)
    }

    /// 分页查询用户
    pub async fn find_page(pool: &SqlitePool, page: i64, page_size: i64) -> DaoResult<Vec<User>> {
        if page < 1 || page_size < 1 {
            return Err(DaoError::InvalidParameter(
                "页码和每页数量必须大于 0".to_string(),
            ));
        }

        let offset = (page - 1) * page_size;
        let users = sqlx::query_as::<_, User>(
            "SELECT id, name, account, password, role, created_at, updated_at, deleted_at FROM t_user WHERE deleted_at IS NULL ORDER BY id LIMIT ? OFFSET ?"
        )
        .bind(page_size)
        .bind(offset)
        .fetch_all(pool)
        .await?;

        Ok(users)
    }

    /// 统计用户数量
    pub async fn count(pool: &SqlitePool) -> DaoResult<i64> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM t_user WHERE deleted_at IS NULL")
                .fetch_one(pool)
                .await?;

        Ok(count)
    }

    /// 更新用户信息
    pub async fn update(
        pool: &SqlitePool,
        id: u32,
        name: Option<&str>,
        password: Option<&str>,
        role: Option<Role>,
    ) -> DaoResult<u64> {
        let mut query = String::from("UPDATE t_user SET updated_at = datetime('now')");
        let mut bindings = Vec::new();

        if let Some(n) = name {
            query.push_str(", name = ?");
            bindings.push(n.to_string());
        }

        if let Some(p) = password {
            query.push_str(", password = ?");
            bindings.push(p.to_string());
        }

        query.push_str(" WHERE id = ?");

        let mut q = sqlx::query(&query);
        for binding in bindings {
            q = q.bind(binding);
        }
        if let Some(r) = role {
            q = q.bind(r);
        }
        q = q.bind(id);

        let result = q.execute(pool).await?;

        if result.rows_affected() == 0 {
            return Err(DaoError::NotFound(format!("用户 ID {} 不存在", id)));
        }

        Ok(result.rows_affected())
    }

    /// 更新密码
    pub async fn update_password(pool: &SqlitePool, id: u32, password: &str) -> DaoResult<u64> {
        let result = sqlx::query(
            "UPDATE t_user SET password = ?, updated_at = datetime('now') WHERE id = ? AND deleted_at IS NULL"
        )
        .bind(password)
        .bind(id)
        .execute(pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(DaoError::NotFound(format!("用户 ID {} 不存在", id)));
        }

        Ok(result.rows_affected())
    }

    /// 软删除用户
    pub async fn soft_delete(pool: &SqlitePool, id: u32) -> DaoResult<u64> {
        let result = sqlx::query(
            "UPDATE t_user SET deleted_at = datetime('now') WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(id)
        .execute(pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(DaoError::NotFound(format!("用户 ID {} 不存在或已删除", id)));
        }

        Ok(result.rows_affected())
    }

    /// 硬删除用户
    pub async fn delete(pool: &SqlitePool, id: u32) -> DaoResult<u64> {
        let result = sqlx::query("DELETE FROM t_user WHERE id = ?")
            .bind(id)
            .execute(pool)
            .await?;

        if result.rows_affected() == 0 {
            return Err(DaoError::NotFound(format!("用户 ID {} 不存在", id)));
        }

        Ok(result.rows_affected())
    }

    /// 检查账号是否存在
    pub async fn exists_by_account(pool: &SqlitePool, account: &str) -> DaoResult<bool> {
        let (count,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM t_user WHERE account = ? AND deleted_at IS NULL")
                .bind(account)
                .fetch_one(pool)
                .await?;

        Ok(count > 0)
    }

    /// 验证用户登录
    pub async fn verify_login(
        pool: &SqlitePool,
        account: &str,
        password: &str,
    ) -> DaoResult<Option<User>> {
        let user = sqlx::query_as::<_, User>(
            "SELECT id, name, account, password, role, created_at, updated_at, deleted_at FROM t_user WHERE account = ? AND password = ? AND deleted_at IS NULL"
        )
        .bind(account)
        .bind(password)
        .fetch_optional(pool)
        .await?;

        Ok(user)
    }
}
