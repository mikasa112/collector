use std::sync::atomic::{
    AtomicBool, AtomicU8, AtomicU64,
    Ordering::{self, Relaxed},
};

use serde::{Deserialize, Serialize};

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
pub enum RunMode {
    //计划自动
    PlanAuto = 0,
    //总功率
    TotalPower = 1,
}

impl TryFrom<u8> for RunMode {
    type Error = RuntimeEmuError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(RunMode::PlanAuto),
            1 => Ok(RunMode::TotalPower),
            _ => Err(RuntimeEmuError::EmuPermissionError),
        }
    }
}

#[repr(u8)]
#[derive(Clone, Copy)]
pub enum ControlSource {
    //本地
    Local = 0,
    //远程
    Remote = 1,
}

impl TryFrom<u8> for ControlSource {
    type Error = RuntimeEmuError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(ControlSource::Local),
            1 => Ok(ControlSource::Remote),
            _ => Err(RuntimeEmuError::EmuPermissionError),
        }
    }
}

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
}

impl Default for SocProtect {
    fn default() -> Self {
        Self::new()
    }
}

/// 防逆流、需量保护的运行参数，默认全部关闭。
///
/// 参数用原子量存放，修改后立即生效，无需重启；修改后由 [`RuntimeEmu::save`] 落盘到
/// `config/emu_runtime_config.json`。功率单位均为 kW。
#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerGuardConfig {
    anti_backflow_enable: AtomicBool,
    /// 允许的最大反送功率（>= 0，0 即零逆流）
    anti_backflow_threshold: AtomicF64,
    /// 防逆流功率回差（> 0）
    anti_backflow_hysteresis: AtomicF64,
    demand_guard_enable: AtomicBool,
    /// 需量限制：关口取电功率上限（> 需量回差）
    demand_limit: AtomicF64,
    /// 需量保护功率回差（0 < 回差 < 需量限制）
    demand_hysteresis: AtomicF64,
}

impl Default for PowerGuardConfig {
    fn default() -> Self {
        Self {
            anti_backflow_enable: AtomicBool::new(false),
            anti_backflow_threshold: AtomicF64::new(0.0),
            anti_backflow_hysteresis: AtomicF64::new(2.0),
            demand_guard_enable: AtomicBool::new(false),
            demand_limit: AtomicF64::new(100.0),
            demand_hysteresis: AtomicF64::new(5.0),
        }
    }
}

/// 一次性修改多个参数，`None` 表示不改
#[derive(Debug, Default, Clone, Copy)]
pub struct PowerGuardUpdate {
    pub anti_backflow_enable: Option<bool>,
    pub anti_backflow_threshold: Option<f64>,
    pub anti_backflow_hysteresis: Option<f64>,
    pub demand_guard_enable: Option<bool>,
    pub demand_limit: Option<f64>,
    pub demand_hysteresis: Option<f64>,
}

impl PowerGuardConfig {
    pub fn anti_backflow_enable(&self) -> bool {
        self.anti_backflow_enable.load(Relaxed)
    }
    pub fn anti_backflow_threshold(&self) -> f64 {
        self.anti_backflow_threshold.load(Relaxed)
    }
    pub fn anti_backflow_hysteresis(&self) -> f64 {
        self.anti_backflow_hysteresis.load(Relaxed)
    }
    pub fn demand_guard_enable(&self) -> bool {
        self.demand_guard_enable.load(Relaxed)
    }
    pub fn demand_limit(&self) -> f64 {
        self.demand_limit.load(Relaxed)
    }
    pub fn demand_hysteresis(&self) -> f64 {
        self.demand_hysteresis.load(Relaxed)
    }

    pub fn set_anti_backflow_enable(&self, on: bool) {
        self.anti_backflow_enable.store(on, Relaxed);
    }
    pub fn set_demand_guard_enable(&self, on: bool) {
        self.demand_guard_enable.store(on, Relaxed);
    }

    pub fn set_anti_backflow_threshold(&self, v: f64) -> Result<(), String> {
        self.update(PowerGuardUpdate {
            anti_backflow_threshold: Some(v),
            ..Default::default()
        })
    }

    pub fn set_anti_backflow_hysteresis(&self, v: f64) -> Result<(), String> {
        self.update(PowerGuardUpdate {
            anti_backflow_hysteresis: Some(v),
            ..Default::default()
        })
    }

    pub fn set_demand_limit(&self, v: f64) -> Result<(), String> {
        self.update(PowerGuardUpdate {
            demand_limit: Some(v),
            ..Default::default()
        })
    }

    pub fn set_demand_hysteresis(&self, v: f64) -> Result<(), String> {
        self.update(PowerGuardUpdate {
            demand_hysteresis: Some(v),
            ..Default::default()
        })
    }

    /// 一次性修改多个参数：先用"当前值 + 修改值"整体校验，通过后才写入，
    /// 因此同时修改需量限制和回差时不受先后顺序影响，校验失败则一个都不改
    pub fn update(&self, u: PowerGuardUpdate) -> Result<(), String> {
        let threshold = u
            .anti_backflow_threshold
            .unwrap_or_else(|| self.anti_backflow_threshold());
        let ab_hyst = u
            .anti_backflow_hysteresis
            .unwrap_or_else(|| self.anti_backflow_hysteresis());
        let limit = u.demand_limit.unwrap_or_else(|| self.demand_limit());
        let d_hyst = u
            .demand_hysteresis
            .unwrap_or_else(|| self.demand_hysteresis());

        if !threshold.is_finite() || threshold < 0.0 {
            return Err(format!("防逆流功率阈值须 >= 0, 收到 {threshold}"));
        }
        if !ab_hyst.is_finite() || ab_hyst <= 0.0 {
            return Err(format!("防逆流功率回差须 > 0, 收到 {ab_hyst}"));
        }
        if !limit.is_finite() || !d_hyst.is_finite() || d_hyst <= 0.0 || d_hyst >= limit {
            return Err(format!(
                "需量保护须满足 0 < 功率回差 < 需量限制, 收到 回差 {d_hyst}, 限制 {limit}"
            ));
        }

        self.anti_backflow_threshold.store(threshold, Relaxed);
        self.anti_backflow_hysteresis.store(ab_hyst, Relaxed);
        self.demand_limit.store(limit, Relaxed);
        self.demand_hysteresis.store(d_hyst, Relaxed);
        if let Some(on) = u.anti_backflow_enable {
            self.set_anti_backflow_enable(on);
        }
        if let Some(on) = u.demand_guard_enable {
            self.set_demand_guard_enable(on);
        }
        Ok(())
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
    #[serde(skip)]
    run_mode: AtomicU8,
    #[serde(skip)]
    control_source: AtomicU8,
    pub soc_protect: SocProtect,
    pub power_guard: PowerGuardConfig,
}

/// 配置文件内容：SOC 保护与功率保护合并存放，缺失的段使用默认值
#[derive(Deserialize)]
struct PersistedConfig {
    #[serde(default)]
    soc_protect: SocProtect,
    #[serde(default)]
    power_guard: PowerGuardConfig,
}

impl PersistedConfig {
    /// 解析配置文件内容，为空或解析失败时使用默认值；
    /// 兼容合并前只有 SOC 限制的旧格式（顶层即 charge_limit/discharge_limit）
    fn parse(content: &str) -> Self {
        let default = || Self {
            soc_protect: SocProtect::default(),
            power_guard: PowerGuardConfig::default(),
        };
        if content.trim().is_empty() {
            return default();
        }
        match serde_json::from_str::<serde_json::Value>(content) {
            Ok(v) if v.get("soc_protect").is_none() && v.get("charge_limit").is_some() => {
                Self {
                    soc_protect: serde_json::from_value(v).unwrap_or_default(),
                    power_guard: PowerGuardConfig::default(),
                }
            }
            Ok(v) => serde_json::from_value(v).unwrap_or_else(|err| {
                tracing::warn!("[EMU] 解析运行配置失败, 使用默认配置: {}", err);
                default()
            }),
            Err(err) => {
                tracing::warn!("[EMU] 解析运行配置失败, 使用默认配置: {}", err);
                default()
            }
        }
    }
}

impl RuntimeEmu {
    pub async fn new() -> std::io::Result<Self> {
        // 不能以 truncate 方式打开：那会在读取前清空文件，SOC 限制重启后就永远回到默认值
        let content = match tokio::fs::read_to_string(EMU_RUNTIME_CONFIG).await {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(err) => return Err(err),
        };
        let PersistedConfig {
            soc_protect,
            power_guard,
        } = PersistedConfig::parse(&content);

        let runtime = Self {
            permission: AtomicU8::new(3),
            operation_mode: AtomicU8::new(0),
            health: AtomicU8::new(2),
            run_mode: AtomicU8::new(1),
            control_source: AtomicU8::new(0),
            soc_protect,
            power_guard,
        };
        runtime.save().await?;
        Ok(runtime)
    }

    /// 把 SOC 保护和功率保护参数一并落盘到 `config/emu_runtime_config.json`
    pub async fn save(&self) -> std::io::Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        tokio::fs::write(EMU_RUNTIME_CONFIG, json).await
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

    pub fn run_mode(&self) -> Result<RunMode, RuntimeEmuError> {
        let r = self.run_mode.load(Relaxed);
        let rm = RunMode::try_from(r)?;
        Ok(rm)
    }

    pub fn set_run_mode(&self, mode: RunMode) {
        self.run_mode.store(mode as u8, Relaxed);
    }

    pub fn control_source(&self) -> Result<ControlSource, RuntimeEmuError> {
        let c = self.control_source.load(Relaxed);
        let cs = ControlSource::try_from(c)?;
        Ok(cs)
    }

    pub fn set_control_source(&self, source: ControlSource) {
        self.control_source.store(source as u8, Relaxed);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soc_protect_keeps_saved_values_across_restart() {
        let saved = SocProtect::new();
        saved.set_charge_limit(90.0);
        saved.set_discharge_limit(10.0);
        let json = serde_json::to_string_pretty(&serde_json::json!({ "soc_protect": saved }))
            .unwrap();

        let loaded = PersistedConfig::parse(&json).soc_protect;
        assert_eq!(
            (loaded.charge_limit(), loaded.discharge_limit()),
            (90.0, 10.0)
        );
    }

    #[test]
    fn soc_protect_falls_back_to_default() {
        for content in ["", "  \n", "not json"] {
            let p = PersistedConfig::parse(content).soc_protect;
            assert_eq!((p.charge_limit(), p.discharge_limit()), (95.0, 5.0));
        }
    }

    #[test]
    fn legacy_soc_only_config_is_migrated() {
        let c = PersistedConfig::parse(r#"{"charge_limit": 90.0, "discharge_limit": 10.0}"#);
        assert_eq!(
            (c.soc_protect.charge_limit(), c.soc_protect.discharge_limit()),
            (90.0, 10.0)
        );
        assert!(!c.power_guard.anti_backflow_enable());
    }

    #[test]
    fn power_guard_update_validates_as_a_whole() {
        let c = PowerGuardConfig::default(); // 限值100 回差5
        assert!(c.set_anti_backflow_threshold(-1.0).is_err());
        assert!(c.set_anti_backflow_threshold(0.0).is_ok());
        assert!(c.set_anti_backflow_hysteresis(0.0).is_err());
        assert!(c.set_demand_hysteresis(100.0).is_err()); // 不得 >= 需量限制
        assert!(c.set_demand_limit(5.0).is_err()); // 不得 <= 需量回差

        // 同时改限值和回差：单独改任何一个都会失败，整体校验则通过
        let both = PowerGuardUpdate {
            demand_limit: Some(8.0),
            demand_hysteresis: Some(6.0),
            ..Default::default()
        };
        assert!(c.update(both).is_ok());
        assert_eq!((c.demand_limit(), c.demand_hysteresis()), (8.0, 6.0));
    }

    #[test]
    fn power_guard_update_is_all_or_nothing() {
        let c = PowerGuardConfig::default();
        let bad = PowerGuardUpdate {
            anti_backflow_enable: Some(true),
            anti_backflow_threshold: Some(3.0),
            demand_hysteresis: Some(-1.0),
            ..Default::default()
        };
        assert!(c.update(bad).is_err());
        assert!(!c.anti_backflow_enable());
        assert_eq!(c.anti_backflow_threshold(), 0.0);
    }
}
