use salvo::{Request, handler};
use validator::Validate;

use crate::{
    core::{ApiResult, response::ObjResponse},
    services::user::UserService,
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

#[handler]
pub async fn login(req: &mut Request) -> ApiResult<ObjResponse<String>> {
    let params = req.parse_json::<LoginParams>().await?;
    params.validate()?;
    let user_service = UserService::new()?;
    let token = user_service.login(params).await?;
    Ok(ObjResponse::ok(token))
}

#[handler]
pub async fn create_user(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<CreateUserParams>().await?;
    params.validate()?;
    let user_service = UserService::new()?;
    user_service.create_user(params).await?;
    Ok(ObjResponse::ok(()))
}
