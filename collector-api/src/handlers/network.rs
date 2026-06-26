use salvo::{Request, handler};
use validator::Validate;

use crate::{
    core::{
        ApiResult,
        response::{ListResponse, ObjResponse},
    },
    services::network::{NetworkService, WifiDev},
};

#[handler]
pub async fn scan() -> ApiResult<ListResponse<WifiDev>> {
    let service = NetworkService::new()?;
    let list = service.scan().await?;
    Ok(ListResponse::ok(list))
}

#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct ConnectWifiParams {
    #[validate(length(min = 1, message = "SSID不能为空"))]
    pub ssid: String,
    /// WPA/WPA2 密码，留空或不传表示开放网络
    #[validate(length(min = 8, message = "密码至少8位"))]
    pub password: Option<String>,
}

#[handler]
pub async fn connect(req: &mut Request) -> ApiResult<ObjResponse<()>> {
    let params = req.parse_json::<ConnectWifiParams>().await?;
    params.validate()?;
    let service = NetworkService::new()?;
    service.connect(params.ssid, params.password).await?;
    Ok(ObjResponse::ok(()))
}
