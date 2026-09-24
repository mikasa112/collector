use salvo::{Request, handler};
use validator::Validate;

use crate::{
    core::{
        ApiResult,
        response::{ListResponse, ObjResponse},
    },
    services::system::{BackupInfo, SystemService, UnitStatus},
};

#[handler]
pub async fn get_config() -> ApiResult<ObjResponse<String>> {
    let service = SystemService::new()?;
    let content = service.read_config().await?;
    Ok(ObjResponse::ok(content))
}

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct UpdateConfigParams {
    #[validate(length(min = 1, message = "配置内容不能为空"))]
    pub content: String,
}

#[handler]
pub async fn put_config(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<UpdateConfigParams>().await?;
    params.validate()?;
    let service = SystemService::new()?;
    service.update_config(params.content).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn list_backups() -> ApiResult<ListResponse<BackupInfo>> {
    let service = SystemService::new()?;
    let list = service.list_backups().await?;
    let len = list.len();
    Ok(ListResponse::ok(list, len))
}

#[handler]
pub async fn restore_backup(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let name = req
        .param::<String>("name")
        .ok_or_else(|| crate::services::ServiceError::invalid_parameter("缺少备份文件名"))?;
    let service = SystemService::new()?;
    service.restore_backup(&name).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn restart() -> ApiResult<ObjResponse<()>> {
    let service = SystemService::new()?;
    service.restart().await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn status() -> ApiResult<ObjResponse<UnitStatus>> {
    let service = SystemService::new()?;
    let unit_status = service.status().await?;
    Ok(ObjResponse::ok(unit_status))
}
