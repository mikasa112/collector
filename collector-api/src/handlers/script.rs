use salvo::{Request, handler};
use serde::Deserialize;

use crate::{
    core::{
        ApiResult,
        response::{ListResponse, ObjResponse},
    },
    services::script::{ScriptEntry, ScriptService},
};

#[handler]
pub async fn list_scripts() -> ApiResult<ListResponse<ScriptEntry>> {
    let service = ScriptService::new().await?;
    let list = service.list_tree().await?;
    let len = list.len();
    Ok(ListResponse::ok(list, len))
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScriptPathQuery {
    pub path: String,
}

#[handler]
pub async fn get_script(req: &mut Request) -> ApiResult<ObjResponse<String>> {
    let query = req.parse_queries::<ScriptPathQuery>()?;
    let service = ScriptService::new().await?;
    let content = service.read_file(&query.path).await?;
    Ok(ObjResponse::ok(content))
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScriptContentParams {
    pub path: String,
    pub content: String,
}

/// 保存已存在的脚本
#[handler]
pub async fn put_script(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<ScriptContentParams>().await?;
    let service = ScriptService::new().await?;
    service.save_file(&params.path, &params.content).await?;
    Ok(ObjResponse::ok(()))
}

/// 新建脚本，目标已存在则报错
#[handler]
pub async fn post_script(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<ScriptContentParams>().await?;
    let service = ScriptService::new().await?;
    service.create_file(&params.path, &params.content).await?;
    Ok(ObjResponse::ok(()))
}

#[handler]
pub async fn delete_script(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let query = req.parse_queries::<ScriptPathQuery>()?;
    let service = ScriptService::new().await?;
    service.delete_file(&query.path).await?;
    Ok(ObjResponse::ok(()))
}
