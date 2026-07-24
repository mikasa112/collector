mod cmd;
pub mod core;
mod emu_runtime;
mod fault;
mod planned_curve;
mod tms;

// EMU功能点位常量定义
// EMU功能点位常量定义
pub(crate) const ID_OPERATION_MODE: u32 = 1;
pub(crate) const KEY_OPERATION_MODE: &str = "operation_mode";

pub(crate) const ID_PERMISSION: u32 = 2;
pub(crate) const KEY_PERMISSION: &str = "permission";

pub(crate) const ID_HEALTH_STATUS: u32 = 3;
pub(crate) const KEY_HEALTH_STATUS: &str = "health_status";

pub(crate) const ID_CHARGE_SOC_LIMIT: u32 = 4;
pub(crate) const KEY_CHARGE_SOC_LIMIT: &str = "charge_soc_limit";

pub(crate) const ID_DISCHARGE_SOC_LIMIT: u32 = 5;
pub(crate) const KEY_DISCHARGE_SOC_LIMIT: &str = "discharge_soc_limit";

pub(crate) const ID_PLANNED_CURVE: u32 = 6;
pub(crate) const KEY_PLANNED_CURVE: &str = "planned_curve";

pub(crate) const ID_SYS_TMS_MODE: u32 = 10;
pub(crate) const KEY_SYS_TMS_MODE: &str = "sys_tms_mode";
