pub mod code;
pub mod response;

pub type ApiResult<T> = Result<T, code::Code>;
