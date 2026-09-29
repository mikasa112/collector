use salvo::{Depot, Request, handler};
use validator::Validate;

use crate::{
    core::{
        ApiResult,
        response::{ListResponse, ObjResponse},
    },
    middleware::auth::current_role,
    models::user::UserSummary,
    services::{ServiceError, user::UserService},
};

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct LoginParams {
    #[validate(length(min = 1, message = "UserName不能为空"))]
    pub username: String,
    #[validate(length(min = 1, message = "Password不能为空"))]
    pub password: String,
}

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct CreateUserParams {
    pub name: Option<String>,
    #[validate(length(min = 1, message = "Username不能为空"))]
    pub username: String,
    #[validate(length(min = 1, message = "Password不能为空"))]
    pub password: String,
    #[validate(length(min = 1, message = "Role不能为空"))]
    pub role: String,
}

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct UpdateUserParams {
    pub name: Option<String>,
    #[validate(length(min = 6, message = "Password长度不能少于6位"))]
    pub password: Option<String>,
    pub role: Option<String>,
}

#[handler]
pub async fn login(req: &mut Request) -> ApiResult<ObjResponse<String>> {
    let params = req.parse_json::<LoginParams>().await?;
    params.validate()?;
    let user_service = UserService::new()?;
    let token = user_service.login(params).await?;
    Ok(ObjResponse::ok(token))
}

#[handler]
pub async fn create_user(req: &mut Request, depot: &mut Depot) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<CreateUserParams>().await?;
    params.validate()?;
    let caller_role =
        current_role(depot).ok_or_else(|| ServiceError::auth_failed("未登录或登录已过期"))?;
    let user_service = UserService::new()?;
    user_service.create_user(caller_role, params).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn list_users() -> ApiResult<ListResponse<UserSummary>> {
    let user_service = UserService::new()?;
    let users = user_service.list_users().await?;
    let len = users.len();
    Ok(ListResponse::ok(users, len))
}

#[handler]
pub async fn update_user(req: &mut Request, depot: &mut Depot) -> ApiResult<ObjResponse<()>> {
    let id = req
        .param::<u32>("id")
        .ok_or_else(|| ServiceError::invalid_parameter("缺少用户 ID"))?;
    let params = req.parse_json::<UpdateUserParams>().await?;
    params.validate()?;
    let caller_role =
        current_role(depot).ok_or_else(|| ServiceError::auth_failed("未登录或登录已过期"))?;
    let user_service = UserService::new()?;
    user_service.update_user(caller_role, id, params).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn delete_user(req: &mut Request, depot: &mut Depot) -> ApiResult<ObjResponse<()>> {
    let id = req
        .param::<u32>("id")
        .ok_or_else(|| ServiceError::invalid_parameter("缺少用户 ID"))?;
    let caller_role =
        current_role(depot).ok_or_else(|| ServiceError::auth_failed("未登录或登录已过期"))?;
    let user_service = UserService::new()?;
    user_service.delete_user(caller_role, id).await?;
    Ok(ObjResponse::ok(()))
}
