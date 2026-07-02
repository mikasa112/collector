use std::time::Duration;

use crate::strategy::{Schedule, Strategy, StrategyError};
use async_trait::async_trait;
use collector_core::{
    center::{DataCenterError, SharedPointCenter},
    core::point::Val,
    down,
};

#[derive(Debug)]
enum TmsMode {
    Cooling = 1,
    Heating = 2,
    Circulation = 3,
}

pub struct Tms {
    center: SharedPointCenter,
}

impl Tms {
    pub fn new(center: SharedPointCenter) -> Self {
        Self { center }
    }
}

impl Tms {
    async fn set_enable(&self, enable: bool) -> Result<(), DataCenterError> {
        self.center
            .dispatch("tms", vec![down!(id: 2000, Val::U8(enable as u8))])
            .await?;
        Ok(())
    }

    async fn set_mode(&self, mode: TmsMode) -> Result<(), DataCenterError> {
        self.center
            .dispatch("tms", vec![down!(id: 2001, Val::U8(mode as u8))])
            .await?;
        Ok(())
    }

    async fn set_outlet_cooling_temp(&self, temp: f64) -> Result<(), DataCenterError> {
        self.center
            .dispatch("tms", vec![down!(id: 2002, Val::F64(temp))])
            .await?;
        Ok(())
    }

    async fn set_outlet_heating_temp(&self, temp: f64) -> Result<(), DataCenterError> {
        self.center
            .dispatch("tms", vec![down!(id: 2004, Val::F64(temp))])
            .await?;
        Ok(())
    }

    async fn fallback_cooling(&self) -> Result<(), StrategyError> {
        self.set_mode(TmsMode::Cooling).await?;
        self.set_outlet_cooling_temp(22.0).await?;
        Ok(())
    }
}

#[async_trait]
impl Strategy for Tms {
    fn name(&self) -> &str {
        "热管理"
    }

    fn schedule(&self) -> Schedule {
        Schedule::Interval(Duration::from_secs(30))
    }

    async fn on_tick(&mut self) -> Result<(), StrategyError> {
        let bcu_comm = self.center.read("bcu", 0xFFFF);
        if let Some(bcu_comm) = bcu_comm
            && let Ok(v) = u32::try_from(bcu_comm.value)
            && v == 1
        {
            self.fallback_cooling().await?;
            tracing::info!("[热管理] -> 本地模式 BCU通讯断开，执行制冷模式，出水温度22°C");
            return Ok(());
        }

        let t_max = self
            .center
            .read("bcu", 19)
            .and_then(|p| f64::try_from(p.value).ok());
        let t_min = self
            .center
            .read("bcu", 23)
            .and_then(|p| f64::try_from(p.value).ok());
        let t_vag = self
            .center
            .read("bcu", 27)
            .and_then(|p| f64::try_from(p.value).ok());
        let (Some(t_max), Some(t_min), Some(t_vag)) = (t_max, t_min, t_vag) else {
            self.fallback_cooling().await?;
            tracing::info!("[热管理] -> 本地模式 温度数据异常，执行制冷模式，出水温度22°C");
            return Ok(());
        };

        if (25.0..28.0).contains(&t_max) && (22.0..28.0).contains(&t_vag) {
            self.set_enable(true).await?;
            self.set_mode(TmsMode::Circulation).await?;
            tracing::info!("[热管理] -> 自循环模式 机组仅水泵运行");
        } else if t_min <= 10.0 && t_vag <= 15.0 {
            self.set_enable(true).await?;
            self.set_mode(TmsMode::Heating).await?;
            self.set_outlet_heating_temp(15.0).await?;
            tracing::info!("[热管理] -> 制热模式 出水温度15°C");
        } else if (28.0..34.0).contains(&t_max) && t_vag >= 26.0 {
            self.set_enable(true).await?;
            self.set_mode(TmsMode::Cooling).await?;
            self.set_outlet_cooling_temp(24.0).await?;
            tracing::info!("[热管理] -> 一级制冷 出水温度24°C");
        } else if t_max >= 34.0 && t_vag >= 28.0 {
            self.set_enable(true).await?;
            self.set_mode(TmsMode::Cooling).await?;
            self.set_outlet_cooling_temp(22.0).await?;
            tracing::info!("[热管理] -> 二级制冷 出水温度22°C");
        } else {
            self.set_enable(false).await?;
            tracing::info!("[热管理] -> 待机");
        }

        Ok(())
    }
}
