use std::{future::Future, pin::Pin};

use collector_core::{
    center::{DataCenterError, SharedPointCenter},
    core::point::{DownDataPoint, Val},
    down,
};

use crate::{DataDriven, strategy::StrategyError};

pub type CommandFunc =
    Box<dyn Fn(SharedPointCenter) -> Pin<Box<dyn Future<Output = Result<(), CommandError>>>>>;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    CenterError(#[from] DataCenterError),
    #[error("{0}")]
    New(String),
}

pub trait Command: crate::DataDriven + Send + Sync + 'static {
    fn name(&self) -> String;
    fn func(&self) -> CommandFunc;
}

pub struct PowerOn;

impl Command for PowerOn {
    fn name(&self) -> String {
        String::from("系统上电")
    }

    fn func(&self) -> CommandFunc {
        Box::new(
            |center: SharedPointCenter| -> Pin<Box<dyn Future<Output = Result<(), CommandError>>>> {
                tracing::info!("[上电] 执行中...");
                Box::pin(async move {
                    //bcu 启动
                    center
                        .dispatch("bcu", vec![down!(id: 3, Val::U8(0x1))])
                        .await?;
                    //等待PCS上电，建立通信
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
                    const MAX_TRIES: usize = 10;
                    let mut tries = 0;
                    loop {
                        ticker.tick().await;
                        if let Some(comm) = center.read("pcs", 0xFFFF)
                            && comm.value == Val::U8(0)
                        {
                            break;
                        }
                        tries += 1;
                        if tries >= MAX_TRIES {
                            tracing::info!("[上电] 失败, PCS上电超时!");
                            return Err(CommandError::New("PCS上电超时".to_string()));
                        }
                    }
                    // 1. 设置远程
                    // 2. 清除故障
                    // 3. 开机指令
                    center
                        .dispatch(
                            "pcs",
                            vec![
                                down!(id: 3000, Val::U8(1)),
                                down!(id: 3001, Val::U8(1)),
                                down!(id: 3006, Val::U8(1)),
                            ],
                        )
                        .await?;
                    tracing::info!("[上电] 成功");
                    Ok(())
                })
            },
        )
    }
}

#[async_trait::async_trait]
impl DataDriven for PowerOn {
    async fn down(&self, _points: &[DownDataPoint]) -> Result<(), StrategyError> {
        Ok(())
    }
}
