use std::sync::atomic::{
    AtomicU8, AtomicU64,
    Ordering::{self, Relaxed},
};

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
}

pub struct RuntimeEmu {
    permission: AtomicU8,
    operation_mode: AtomicU8,
    health: AtomicU8,
    pub soc_protect: SocProtect,
}

impl Default for RuntimeEmu {
    fn default() -> Self {
        Self::new()
    }
}

impl RuntimeEmu {
    pub fn new() -> Self {
        Self {
            permission: AtomicU8::new(3),
            operation_mode: AtomicU8::new(0),
            health: AtomicU8::new(2),
            soc_protect: SocProtect::new(),
        }
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
