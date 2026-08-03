use collector_core::{
    center::{DataCenterError, SharedPointCenter},
    core::point::{DownDataPoint, PointRef, Val},
    down,
};

use crate::{
    DataDriven,
    emu::{ID_EMU_POWER, KEY_EMU_POWER},
    strategy::StrategyError,
};

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    CenterError(#[from] DataCenterError),
    #[error("{0}")]
    New(String),
}

pub trait Command: crate::DataDriven + Send + Sync + 'static {
    fn name(&self) -> &str;
}

pub struct EmuPower {
    center: SharedPointCenter,
}

impl EmuPower {
    pub fn new(center: SharedPointCenter) -> Self {
        Self { center }
    }
}

impl EmuPower {
    async fn grid_on_start(&self) -> Result<(), CommandError> {
        // bcu 启动
        self.center
            .dispatch("bcu", vec![down!(id: 3, Val::U8(0x1))])
            .await?;
        //等待PCS上电，建立通信
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        const MAX_TRIES: usize = 30;
        let mut tries = 0;
        loop {
            ticker.tick().await;
            if let Some(comm) = self.center.read("pcs", 0xFFFF)
                && comm.value == Val::U8(0)
            {
                break;
            }
            tries += 1;
            if tries >= MAX_TRIES {
                tracing::info!("[系统并网上电] 失败, PCS上电超时!");
                return Err(CommandError::New("PCS上电超时".to_string()));
            }
        }
        // 1. 设置远程
        self.center
            .dispatch("pcs", vec![down!(id: 3006, Val::U8(1))])
            .await?;
        // 2. 清除故障
        self.center
            .dispatch("pcs", vec![down!(id: 3000, Val::U8(1))])
            .await?;
        // 3. 开机指令
        self.center
            .dispatch("pcs", vec![down!(id: 3001, Val::U8(1))])
            .await?;
        tracing::info!("[系统并网上电] 成功");
        Ok(())
    }
    async fn grid_off_start(&self) -> Result<(), CommandError> {
        // bcu 启动
        self.center
            .dispatch("bcu", vec![down!(id: 3, Val::U8(0x1))])
            .await?;
        //等待PCS上电，建立通信
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(1));
        const MAX_TRIES: usize = 10;
        let mut tries = 0;
        loop {
            ticker.tick().await;
            if let Some(comm) = self.center.read("pcs", 0xFFFF)
                && comm.value == Val::U8(0)
            {
                break;
            }
            tries += 1;
            if tries >= MAX_TRIES {
                tracing::info!("[系统黑启动] 失败, PCS上电超时!");
                return Err(CommandError::New("PCS上电超时".to_string()));
            }
        }
        // 1. 设置远程
        // 2. 清除故障
        // 3. 设置为VF离网模式
        // 4. 设置离网输出给定电压
        // 5. 开机指令
        self.center
            .dispatch(
                "pcs",
                vec![
                    down!(id: 3006, Val::U8(1)),
                    down!(id: 3000, Val::U8(1)),
                    down!(id: 2005, Val::U8(1)),
                    down!(id: 2006, Val::F64(400.0)),
                    down!(id: 3001, Val::F64(400.0)),
                ],
            )
            .await?;
        tracing::info!("[系统黑启动] 成功");
        Ok(())
    }
}

impl Command for EmuPower {
    fn name(&self) -> &str {
        "EMU上电方式"
    }
}

#[async_trait::async_trait]
impl DataDriven for EmuPower {
    async fn down(&self, points: &[DownDataPoint]) -> Result<(), StrategyError> {
        for p in points.iter() {
            if p.point == PointRef::Id(ID_EMU_POWER)
                || p.point == PointRef::Key(KEY_EMU_POWER.to_string())
            {
                let v = p.value.as_u32()?;
                //并网启动
                if v == 1 {
                    let _ = self.grid_on_start().await;
                } else if v == 2 {
                    //黑启动
                    let _ = self.grid_off_start().await;
                }
            }
        }
        Ok(())
    }
}
