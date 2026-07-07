pub mod data;
pub mod error;
#[cfg(target_os = "linux")]
pub mod network;
pub mod planned_curve;
pub mod user;

use collector_core::center::SharedPointCenter;
// Service 层使用独立的错误类型
pub use error::{ServiceError, ServiceResult};
use salvo::Depot;

trait Service {
    fn center(&self, depot: &mut Depot) -> ServiceResult<SharedPointCenter> {
        let center = depot
            .get::<SharedPointCenter>("center")
            .map_err(|_| ServiceError::InternalError(String::from("SharedPointCenter not found")))?
            .clone();
        Ok(center)
    }
}
