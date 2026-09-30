use collector_core::{field::FieldSpec, field_spec};

pub(crate) mod alarm;
mod cmd;
pub mod core;
mod emu_runtime;
mod fault;
mod planned_curve;
pub mod power_guard;
mod taos;
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

pub(crate) const ID_EMU_POWER: u32 = 7;
pub(crate) const KEY_EMU_POWER: &str = "emu_power";

pub(crate) const ID_SYS_TMS_MODE: u32 = 10;
pub(crate) const KEY_SYS_TMS_MODE: &str = "sys_tms_mode";

pub(crate) const ID_RUN_MODE: u32 = 8;
pub(crate) const KEY_RUN_MODE: &str = "run_mode";

pub(crate) const ID_CONTROL_SOURCE: u32 = 9;
pub(crate) const KEY_CONTROL_SOURCE: &str = "control_source";

/// EMU 用到的全部逻辑字段，集中在此声明并统一注入 [`collector_core::field::field_registry`]，
/// 不再由各策略各自声明、逐个注册。
pub(crate) const FIELDS: &[FieldSpec] = &[
    field_spec!("bcu_comm_status", "BCU通信状态", Read, "bcu", Id(34)),
    field_spec!("soc", "SOC", Read, "bcu", Id(32)),
    field_spec!("bcu_current", "BCU电流(负充正放)", Read, "bcu", Id(46)),
    field_spec!(
        "pcs_active_power",
        "PCS有功功率设定",
        Write,
        "pcs",
        Id(2003)
    ),
];
