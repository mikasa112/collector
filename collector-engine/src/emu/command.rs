use std::{future::Future, pin::Pin};

use collector_core::{
    center::{DataCenterError, SharedPointCenter},
    core::point::Val,
    down,
};

pub type CommandFunc =
    Box<dyn Fn(SharedPointCenter) -> Pin<Box<dyn Future<Output = Result<(), CommandError>>>>>;

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    CenterError(#[from] DataCenterError),
    #[error("{0}")]
    New(String),
}

pub struct Command {
    name: String,
    func: CommandFunc,
}

impl Command {
    pub fn power_on() -> Self {
        let func = Box::new(
            |center: SharedPointCenter| -> Pin<Box<dyn Future<Output = Result<(), CommandError>>>> {
                Box::pin(async move {
                    //上高压
                    center
                        .dispatch("bcu", vec![down!(id: 123, Val::U8(1))])
                        .await?;
                    //等待PCS上电，建立通信
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
                    const MAX_TRIES: usize = 10;
                    let mut tries = 0;
                    loop {
                        ticker.tick().await;
                        if let Some(comm) = center.read("pcs", 0xFFFF) {
                            if comm.value == Val::U8(0) {
                                break;
                            }
                        }
                        tries += 1;
                        if tries >= MAX_TRIES {
                            return Err(CommandError::New("PCS上电超时".to_string()));
                        }
                    }
                    // 1. 设置远程
                    // 2. 清除故障
                    // 3. 开机指令
                    center
                        .dispatch(
                            "pcs",
                            vec![down!(id: 3000, Val::U8(1)), down!(id: 3001, Val::U8(1))],
                        )
                        .await?;
                    Ok(())
                })
            },
        );
        Self {
            name: String::from("开机"),
            func,
        }
    }
}
