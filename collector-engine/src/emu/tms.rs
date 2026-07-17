use std::{sync::Arc, time::Duration};

use crate::strategy::{Schedule, Strategy, StrategyError};
use async_trait::async_trait;
use collector_core::{
    center::{DataCenterError, SharedPointCenter},
    core::point::{DataPoint, DownDataPoint, PointRef, Val},
    down,
};
use parking_lot::RwLock;

#[derive(Debug)]
enum TmsMode {
    Cooling = 1,
    Heating = 2,
    Circulation = 3,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SysTmsMode {
    //手动
    Manual = 0,
    //自动
    Auto = 1,
    //自循环
    Circulation = 2,
    //一级制冷
    Level1Cooling = 3,
    //二级制冷
    Level2Cooling = 4,
    //制热
    Heating = 5,
    //待机
    Standby = 6,
}

impl SysTmsMode {
    fn from_value(val: &Val) -> Self {
        match val {
            Val::U8(v) => Self::from_u8(*v),
            _ => Self::Auto,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Manual,
            1 => Self::Auto,
            2 => Self::Circulation,
            3 => Self::Level1Cooling,
            4 => Self::Level2Cooling,
            5 => Self::Heating,
            6 => Self::Standby,
            _ => Self::Auto,
        }
    }
}

pub struct Tms {
    center: SharedPointCenter,
    sys_tms_mode: Arc<RwLock<SysTmsMode>>,
    point: Arc<RwLock<DataPoint>>,
}

impl Tms {
    pub fn new(center: SharedPointCenter) -> Self {
        Self {
            center,
            sys_tms_mode: Arc::new(RwLock::new(SysTmsMode::Auto)),
            point: Arc::new(RwLock::new(DataPoint {
                id: 1,
                key: "sys_tms_mode",
                name: "系统热管理模式",
                value: Val::U8(SysTmsMode::Auto as u8),
                translator: None,
                bits: None,
                words: None,
                unit: None,
            })),
        }
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
    async fn fallback_cooling(&self) -> Result<(), DataCenterError> {
        self.set_mode(TmsMode::Cooling).await?;
        self.set_outlet_cooling_temp(22.0).await?;
        Ok(())
    }
    async fn manual(&self) -> Result<(), DataCenterError> {
        self.push(SysTmsMode::Manual).await?;
        Ok(())
    }
    async fn auto(&self) -> Result<(), DataCenterError> {
        let mode = *self.sys_tms_mode.read();
        if mode != SysTmsMode::Auto {
            return Ok(());
        }
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
            self.circulation().await?;
        } else if t_min <= 10.0 && t_vag <= 15.0 {
            self.heating().await?;
        } else if (28.0..34.0).contains(&t_max) && t_vag >= 26.0 {
            self.level1_cooling().await?;
        } else if t_max >= 34.0 && t_vag >= 28.0 {
            self.level2_cooling().await?;
        } else {
            self.standby().await?;
        }
        self.push(SysTmsMode::Auto).await?;
        Ok(())
    }
    async fn circulation(&self) -> Result<(), DataCenterError> {
        self.set_enable(true).await?;
        self.set_mode(TmsMode::Circulation).await?;
        tracing::info!("[热管理] -> 自循环模式 机组仅水泵运行");
        self.push(SysTmsMode::Circulation).await?;
        Ok(())
    }
    async fn level1_cooling(&self) -> Result<(), DataCenterError> {
        self.set_enable(true).await?;
        self.set_mode(TmsMode::Cooling).await?;
        self.set_outlet_cooling_temp(24.0).await?;
        tracing::info!("[热管理] -> 一级制冷 出水温度24°C");
        self.push(SysTmsMode::Level1Cooling).await?;
        Ok(())
    }
    async fn level2_cooling(&self) -> Result<(), DataCenterError> {
        self.set_enable(true).await?;
        self.set_mode(TmsMode::Cooling).await?;
        self.set_outlet_cooling_temp(22.0).await?;
        tracing::info!("[热管理] -> 二级制冷 出水温度22°C");
        self.push(SysTmsMode::Level2Cooling).await?;
        Ok(())
    }
    async fn heating(&self) -> Result<(), DataCenterError> {
        self.set_enable(true).await?;
        self.set_mode(TmsMode::Heating).await?;
        self.set_outlet_heating_temp(15.0).await?;
        tracing::info!("[热管理] -> 制热模式 出水温度15°C");
        self.push(SysTmsMode::Heating).await?;
        Ok(())
    }
    async fn standby(&self) -> Result<(), DataCenterError> {
        self.set_enable(false).await?;
        tracing::info!("[热管理] -> 待机");
        self.push(SysTmsMode::Standby).await?;
        Ok(())
    }
    async fn push(&self, mode: SysTmsMode) -> Result<(), DataCenterError> {
        let mut point = self.point.read().clone();
        point.value = Val::U8(mode as u8);
        self.center.ingest("emu", vec![point]);
        Ok(())
    }
}

#[async_trait]
impl crate::DataDriven for Tms {
    async fn down(&self, points: &[DownDataPoint]) -> Result<(), DataCenterError> {
        let (id, key) = {
            let point = self.point.read();
            (point.id, point.key)
        };
        for p in points.iter() {
            if PointRef::Id(id) == p.point || PointRef::Key(key.to_string()) == p.point {
                let mode = SysTmsMode::from_value(&p.value);
                match mode {
                    SysTmsMode::Manual => {
                        tracing::info!("[热管理] 手动");
                        self.manual().await?;
                    }
                    SysTmsMode::Auto => {
                        tracing::info!("[热管理] 自动");
                        self.auto().await?;
                    }
                    SysTmsMode::Circulation => {
                        tracing::info!("[热管理] 自循环");
                        self.circulation().await?;
                    }
                    SysTmsMode::Level1Cooling => {
                        tracing::info!("[热管理] 一级制冷");
                        self.level1_cooling().await?;
                    }
                    SysTmsMode::Level2Cooling => {
                        tracing::info!("[热管理] 二级制冷");
                        self.level2_cooling().await?;
                    }
                    SysTmsMode::Heating => {
                        tracing::info!("[热管理] 制热");
                        self.heating().await?;
                    }
                    SysTmsMode::Standby => {
                        tracing::info!("[热管理] 待机");
                        self.standby().await?;
                    }
                }
                *self.sys_tms_mode.write() = mode;
                self.point.write().value = p.value.clone();
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Strategy for Tms {
    fn name(&self) -> &str {
        "热管理"
    }

    async fn on_start(&mut self) -> Result<(), StrategyError> {
        self.push(SysTmsMode::Auto).await?;
        Ok(())
    }

    fn schedule(&self) -> Schedule {
        Schedule::Interval(Duration::from_secs(30))
    }

    async fn on_tick(&mut self) -> Result<(), StrategyError> {
        self.auto().await?;
        Ok(())
    }
}
