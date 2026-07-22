use tokio::sync::OnceCell;

use crate::{
    runtime::{RuntimeError, planned_curve::RuntimePlannedCurve},
    utils::database::get_database,
};

static RUNTIME: OnceCell<Runtime> = OnceCell::const_new();

pub struct Runtime {
    pub planned_curve: RuntimePlannedCurve,
}

impl Runtime {
    async fn new() -> Result<Self, RuntimeError> {
        let pool = get_database().expect("初始化数据库失败");
        let planned_curve = RuntimePlannedCurve::new(pool.clone()).await?;
        Ok(Self { planned_curve })
    }
}

pub async fn get_runtime() -> Result<&'static Runtime, RuntimeError> {
    RUNTIME.get_or_try_init(Runtime::new).await
}
