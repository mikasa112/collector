use std::sync::OnceLock;

use salvo::{
    Depot, FlowCtrl, Handler, Request, Response, Writer, async_trait, handler,
    jwt_auth::{ConstDecoder, HeaderFinder, JwtAuthDepotExt, JwtAuthState},
    prelude::JwtAuth,
};
use serde::{Deserialize, Serialize};

use crate::{core::code::Code, models::user::Role, services::error::ServiceError};

/// 开发环境兜底密钥，生产环境必须通过 COLLECTOR_JWT_SECRET 环境变量覆盖
const DEV_FALLBACK_JWT_SECRET: &[u8] = b"YUANAN008853";

static JWT_SECRET: OnceLock<Vec<u8>> = OnceLock::new();

pub fn jwt_secret() -> &'static [u8] {
    JWT_SECRET
        .get_or_init(|| match std::env::var("COLLECTOR_JWT_SECRET") {
            Ok(secret) if !secret.is_empty() => secret.into_bytes(),
            _ => {
                tracing::warn!(
                    "未设置环境变量 COLLECTOR_JWT_SECRET，正在使用不安全的开发默认密钥，生产环境请务必设置该变量"
                );
                DEV_FALLBACK_JWT_SECRET.to_vec()
            }
        })
        .as_slice()
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct JwtClaims {
    pub username: String,
    pub role: String,
    pub exp: i64,
}

/// 校验 JWT 状态，未通过认证/授权时返回和业务错误一致的 JSON 格式
#[handler]
async fn check_auth_state(
    req: &mut Request,
    depot: &mut Depot,
    res: &mut Response,
    ctrl: &mut FlowCtrl,
) {
    let err = match depot.jwt_auth_state() {
        JwtAuthState::Authorized => return,
        JwtAuthState::Unauthorized => ServiceError::auth_failed("未登录或登录已过期"),
        JwtAuthState::Forbidden => ServiceError::auth_failed("无效的登录凭证"),
    };
    Code::from(err).write(req, depot, res).await;
    ctrl.skip_rest();
}

#[inline]
pub fn auth_handler() -> impl Handler {
    (
        JwtAuth::<JwtClaims, _>::new(ConstDecoder::from_secret(jwt_secret()))
            .finders(vec![Box::new(HeaderFinder::new())])
            .force_passed(true),
        check_auth_state,
    )
}

/// 从已通过 JWT 校验的 claims 中解析出调用者角色
pub fn current_role(depot: &Depot) -> Option<Role> {
    depot
        .jwt_auth_data::<JwtClaims>()
        .and_then(|data| Role::try_from(data.claims.role.as_str()).ok())
}

/// 从已通过 JWT 校验的 claims 中解析出调用者用户名
pub fn current_username(depot: &Depot) -> Option<String> {
    depot
        .jwt_auth_data::<JwtClaims>()
        .map(|data| data.claims.username.clone())
}

/// 校验调用者角色是否达到 min_role，不足则返回 403 并终止后续处理
struct RoleGate {
    min_role: Role,
}

#[async_trait]
impl Handler for RoleGate {
    async fn handle(
        &self,
        req: &mut Request,
        depot: &mut Depot,
        res: &mut Response,
        ctrl: &mut FlowCtrl,
    ) {
        match current_role(depot) {
            Some(role) if role >= self.min_role => {}
            _ => {
                let err = ServiceError::permission_denied("权限不足");
                Code::from(err).write(req, depot, res).await;
                ctrl.skip_rest();
            }
        }
    }
}

/// 要求登录且角色达到 min_role 及以上
pub fn require_role(min_role: Role) -> impl Handler {
    (auth_handler(), RoleGate { min_role })
}
