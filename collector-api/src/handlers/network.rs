use salvo::{Request, handler};
use validator::Validate;

use crate::{
    core::{
        ApiResult,
        response::{ListResponse, ObjResponse},
    },
    services::{
        ethernet::{ApplyResult, DEFAULT_ROLLBACK_SECS, EthernetIface, EthernetService},
        ethernet_cfg::{Ipv4Method, validate_static},
        network::{NetworkService, WifiDev},
    },
};

#[handler]
pub async fn scan() -> ApiResult<ListResponse<WifiDev>> {
    let service = NetworkService::new()?;
    let list = service.scan().await?;
    let len = list.len();
    Ok(ListResponse::ok(list, len))
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

#[handler]
pub async fn ethernet_list() -> ApiResult<ListResponse<EthernetIface>> {
    let service = EthernetService::new()?;
    let list = service.list().await?;
    let len = list.len();
    Ok(ListResponse::ok(list, len))
}

/// 修改有线网口的 IPv4 配置。`method=manual` 时需要 address 和 prefix
#[derive(Debug, Clone, serde::Deserialize, Validate)]
pub struct SetEthernetParams {
    #[validate(length(min = 1, message = "网口名不能为空"))]
    pub name: String,
    pub method: Ipv4Method,
    pub address: Option<String>,
    pub prefix: Option<u32>,
    pub gateway: Option<String>,
    #[serde(default)]
    pub dns: Vec<String>,
    /// 多少秒内需要确认，否则自动回滚，默认60
    #[validate(range(min = 10, max = 300, message = "回滚等待时间须在10-300秒之间"))]
    pub rollback_secs: Option<u64>,
}

#[handler]
pub async fn ethernet_set(req: &mut Request) -> ApiResult<ObjResponse<ApplyResult>> {
    let params = req.parse_json::<SetEthernetParams>().await?;
    params.validate()?;
    let cfg = match params.method {
        Ipv4Method::Manual => {
            let (Some(address), Some(prefix)) = (params.address.as_deref(), params.prefix) else {
                return Err(crate::services::ServiceError::InvalidParameter(
                    "静态IP方式必须提供 address 和 prefix".to_string(),
                )
                .into());
            };
            Some(validate_static(
                address,
                prefix,
                params.gateway.as_deref(),
                &params.dns,
            )?)
        }
        Ipv4Method::Auto => None,
    };
    let service = EthernetService::new()?;
    let result = service
        .apply(
            &params.name,
            params.method,
            cfg,
            params.rollback_secs.unwrap_or(DEFAULT_ROLLBACK_SECS),
        )
        .await?;
    Ok(ObjResponse::ok(result))
}

#[handler]
pub async fn ethernet_confirm() -> ApiResult<ObjResponse<()>> {
    let service = EthernetService::new()?;
    service.confirm().await?;
    Ok(ObjResponse::ok(()))
}
