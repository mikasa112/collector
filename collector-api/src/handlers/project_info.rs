use salvo::{Depot, Request, Response, handler, http::header};
use validator::Validate;

use crate::{
    core::{ApiResult, response::ObjResponse},
    middleware::auth::current_username,
    models::project_info::ProjectInfoView,
    services::{
        ServiceError,
        project_info::{MAX_LOGO_BYTES, ProjectInfoService},
    },
};

#[derive(Debug, serde::Deserialize, Validate)]
pub struct UpdateProjectInfoParams {
    #[validate(length(max = 64, message = "项目名称不能超过64个字符"))]
    pub name: Option<String>,
    #[validate(length(max = 64, message = "项目标题不能超过64个字符"))]
    pub title: Option<String>,
    #[validate(length(max = 32, message = "项目版本不能超过32个字符"))]
    pub version: Option<String>,
    #[validate(range(exclusive_min = 0.0, max = 10_000_000.0, message = "额定功率必须大于0"))]
    pub rated_power_kw: Option<f64>,
    #[validate(range(exclusive_min = 0.0, max = 10_000_000.0, message = "额定能量必须大于0"))]
    pub rated_energy_kwh: Option<f64>,
}

#[handler]
pub async fn get_info(_req: &mut Request) -> ApiResult<ObjResponse<ProjectInfoView>> {
    let service = ProjectInfoService::new()?;
    Ok(ObjResponse::ok(service.get().await?))
}

#[handler]
pub async fn update_info(req: &mut Request, depot: &mut Depot) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<UpdateProjectInfoParams>().await?;
    params.validate()?;
    let service = ProjectInfoService::new()?;
    service.update(params, current_username(depot)).await?;
    Ok(ObjResponse::ok(()))
}

/// multipart 上传，文件字段名为 `logo`
#[handler]
pub async fn upload_logo(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    // 解析上限留出 multipart 边界开销，文件本身大小在 service 中再精确校验
    req.form_data_max_size(MAX_LOGO_BYTES as usize + 16 * 1024)
        .await?;
    let file = req
        .file("logo")
        .await
        .ok_or_else(|| ServiceError::InvalidParameter("缺少logo文件字段".to_string()))?;
    let (path, size) = (file.path().clone(), file.size());
    let service = ProjectInfoService::new()?;
    service.save_logo(&path, size).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn get_logo(res: &mut Response) -> ApiResult<()> {
    let logo = ProjectInfoService::find_logo()
        .await
        .ok_or_else(|| ServiceError::NotFound("尚未上传图标".to_string()))?;
    let bytes = tokio::fs::read(&logo.path)
        .await
        .map_err(ServiceError::from)?;
    // 地址带 ?v=修改时间 参数，更新图标后前端会拿到新地址，因此可放心缓存
    res.add_header(header::CONTENT_TYPE, logo.content_type, true)
        .and_then(|r| r.add_header(header::CACHE_CONTROL, "public, max-age=86400", true))
        .and_then(|r| r.add_header("X-Content-Type-Options", "nosniff", true))
        .map_err(|e| ServiceError::InternalError(e.to_string()))?;
    res.write_body(bytes)
        .map_err(|e| ServiceError::InternalError(e.to_string()))?;
    Ok(())
}

#[handler]
pub async fn delete_logo() -> ApiResult<ObjResponse<()>> {
    let service = ProjectInfoService::new()?;
    service.delete_logo().await?;
    Ok(ObjResponse::ok(()))
}
