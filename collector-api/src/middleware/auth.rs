use salvo::{
    jwt_auth::{ConstDecoder, HeaderFinder},
    prelude::JwtAuth,
};
use serde::{Deserialize, Serialize};

pub const JWT_SECRET: &[u8] = b"YUANXIXI008853";

#[derive(Debug, Serialize, Deserialize)]
pub struct JwtClaims {
    pub username: String,
    pub role: String,
    pub exp: i64,
}

#[inline]
#[allow(dead_code)]
pub fn auth_handler() -> JwtAuth<JwtClaims, ConstDecoder> {
    JwtAuth::new(ConstDecoder::from_secret(JWT_SECRET))
        .finders(vec![Box::new(HeaderFinder::new())])
        .force_passed(false)
}
