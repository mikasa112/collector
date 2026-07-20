use salvo::{
    Depot, FlowCtrl, Handler, Request, Response, Writer, handler,
    jwt_auth::{ConstDecoder, HeaderFinder, JwtAuthDepotExt, JwtAuthState},
    prelude::JwtAuth,
};
use serde::{Deserialize, Serialize};

use crate::{core::code::Code, services::error::ServiceError};

pub const JWT_SECRET: &[u8] = b"YUANAN008853";

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
        JwtAuth::<JwtClaims, _>::new(ConstDecoder::from_secret(JWT_SECRET))
            .finders(vec![Box::new(HeaderFinder::new())])
            .force_passed(true),
        check_auth_state,
    )
}
