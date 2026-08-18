use std::sync::atomic::{
    AtomicU8, AtomicU64,
    Ordering::{self, Relaxed},
};

use serde::{Deserialize, Serialize};
use tokio::{fs::OpenOptions, io::AsyncReadExt};

const EMU_RUNTIME_CONFIG: &str = "./config/emu_runtime_config.json";

#[repr(u8)]
#[derive(Clone, Copy)]
pub enum OperationMode {
    //静置
    Standby = 0,
    //充电中
    Charging = 1,
    //放电中
    Discharging = 2,
}

impl TryFrom<u8> for OperationMode {
    type Error = RuntimeEmuError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(OperationMode::Standby),
            1 => Ok(OperationMode::Charging),
            2 => Ok(OperationMode::Discharging),
            _ => Err(RuntimeEmuError::EmuPermissionError),
        }
    }
}

impl OperationMode {}

#[repr(u8)]
#[derive(Clone, Copy)]
pub enum HealthStatus {
    //正常
    Normal = 0,
    //告警
    Warning = 1,
    //故障
    Alarm = 2,
}

impl TryFrom<u8> for HealthStatus {
    type Error = RuntimeEmuError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(HealthStatus::Normal),
            1 => Ok(HealthStatus::Warning),
            2 => Ok(HealthStatus::Alarm),
            _ => Err(RuntimeEmuError::EmuPermissionError),
        }
    }
}

impl HealthStatus {}

// pub struct EmuState {
//     pub mode: OperationMode,
//     pub health: HealthStatus,
// }

#[repr(u8)]
#[derive(Clone, Copy)]
pub enum EmuPermission {
    //正常
    Normal = 0,
    //禁充
    ChargeDisabled = 1,
    //禁放
    DischargeDisabled = 2,
    //禁充禁放
    TotalStop = 3,
}

#[derive(Debug, thiserror::Error)]
pub enum RuntimeEmuError {
    #[error("`EmuPermission`转换错误")]
    EmuPermissionError,
}

impl TryFrom<u8> for EmuPermission {
    type Error = RuntimeEmuError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(EmuPermission::Normal),
            1 => Ok(EmuPermission::ChargeDisabled),
            2 => Ok(EmuPermission::DischargeDisabled),
            3 => Ok(EmuPermission::TotalStop),
            _ => Err(RuntimeEmuError::EmuPermissionError),
        }
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct SocProtect {
    charge_limit: AtomicF64,
    discharge_limit: AtomicF64,
}

impl SocProtect {
    fn new() -> Self {
        Self {
            charge_limit: AtomicF64::new(95.0f64),
            discharge_limit: AtomicF64::new(5.0f64),
        }
    }

    pub fn charge_limit(&self) -> f64 {
        self.charge_limit.load(Relaxed)
    }

    pub fn set_charge_limit(&self, limit: f64) {
        self.charge_limit.store(limit, Relaxed);
    }
    pub fn discharge_limit(&self) -> f64 {
        self.discharge_limit.load(Relaxed)
    }

    pub fn set_discharge_limit(&self, limit: f64) {
        self.discharge_limit.store(limit, Relaxed);
    }

    pub async fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        tokio::fs::write(EMU_RUNTIME_CONFIG, json).await
    }
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct RuntimeEmu {
    #[serde(skip)]
    permission: AtomicU8,
    #[serde(skip)]
    operation_mode: AtomicU8,
    #[serde(skip)]
    health: AtomicU8,
    pub soc_protect: SocProtect,
}

impl RuntimeEmu {
    pub async fn new() -> std::io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(EMU_RUNTIME_CONFIG)
            .await?;
        let mut content = String::new();
        file.read_to_string(&mut content).await?;

        let soc_protect = if content.trim().is_empty() {
            SocProtect::new()
        } else {
            match serde_json::from_str::<SocProtect>(&content) {
                Ok(soc_protect) => soc_protect,
                Err(err) => {
                    tracing::warn!("[EMU] 解析SOC保护配置失败, 使用默认配置: {}", err);
                    SocProtect::new()
                }
            }
        };

        let runtime = Self {
            permission: AtomicU8::new(3),
            operation_mode: AtomicU8::new(0),
            health: AtomicU8::new(2),
            soc_protect,
        };
        runtime.soc_protect.save().await?;
        Ok(runtime)
    }

    pub fn permission(&self) -> Result<EmuPermission, RuntimeEmuError> {
        let p = self.permission.load(Relaxed);
        let pm = EmuPermission::try_from(p)?;
        Ok(pm)
    }

    pub fn set_permission(&self, p: EmuPermission) {
        self.permission.store(p as u8, Relaxed);
    }

    pub fn operation_mode(&self) -> Result<OperationMode, RuntimeEmuError> {
        let p = self.operation_mode.load(Relaxed);
        let op = OperationMode::try_from(p)?;
        Ok(op)
    }

    pub fn set_operation_mode(&self, mode: OperationMode) {
        self.operation_mode.store(mode as u8, Relaxed);
    }

    pub fn health(&self) -> Result<HealthStatus, RuntimeEmuError> {
        let h = self.health.load(Relaxed);
        let hl = HealthStatus::try_from(h)?;
        Ok(hl)
    }

    pub fn set_health(&self, h: HealthStatus) {
        self.health.store(h as u8, Relaxed);
    }
}

#[derive(Debug)]
pub struct AtomicF64 {
    inner: AtomicU64,
}

impl AtomicF64 {
    pub fn new(value: f64) -> Self {
        Self {
            inner: AtomicU64::new(value.to_bits()),
        }
    }

    pub fn load(&self, order: Ordering) -> f64 {
        f64::from_bits(self.inner.load(order))
    }

    pub fn store(&self, value: f64, order: Ordering) {
        self.inner.store(value.to_bits(), order);
    }
}

impl Serialize for AtomicF64 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_f64(self.load(Ordering::Relaxed))
    }
}

impl<'de> Deserialize<'de> for AtomicF64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = f64::deserialize(deserializer)?;
        Ok(Self::new(value))
    }
}
