use collector_core::{core::point::PointRef, field::FieldSpec, field_spec};

pub(crate) mod alarm;
mod cmd;
pub mod core;
mod emu_runtime;
mod fault;
mod guard_runtime;
mod planned_curve;
pub mod power_guard;
mod taos;
mod tms;

/// EMU 功能点位：ID 与 Key 成对声明，避免两个常量各写各的。
pub(crate) struct Point {
    pub id: u32,
    pub key: &'static str,
}

impl Point {
    /// 下发点是否指向本点位（按 ID 或 Key 匹配）
    pub(crate) fn matches(&self, p: &PointRef) -> bool {
        match p {
            PointRef::Id(id) => *id == self.id,
            PointRef::Key(key) => key == self.key,
            _ => false,
        }
    }
}

macro_rules! point {
    ($name:ident, $id:expr, $key:expr) => {
        pub(crate) const $name: Point = Point { id: $id, key: $key };
    };
}

// EMU功能点位常量定义
point!(OPERATION_MODE, 1, "operation_mode");
point!(PERMISSION, 2, "permission");
point!(HEALTH_STATUS, 3, "health_status");
point!(CHARGE_SOC_LIMIT, 4, "charge_soc_limit");
point!(DISCHARGE_SOC_LIMIT, 5, "discharge_soc_limit");
point!(PLANNED_CURVE, 6, "planned_curve");
point!(EMU_POWER, 7, "emu_power");
point!(RUN_MODE, 8, "run_mode");
point!(CONTROL_SOURCE, 9, "control_source");
point!(SYS_TMS_MODE, 10, "sys_tms_mode");
// 防逆流、需量保护参数（可读可写，由 guard_runtime 处理并回读）
point!(ANTI_BACKFLOW_ENABLE, 11, "anti_backflow_enable");
point!(ANTI_BACKFLOW_THRESHOLD, 12, "anti_backflow_threshold");
point!(ANTI_BACKFLOW_HYSTERESIS, 13, "anti_backflow_hysteresis");
point!(DEMAND_GUARD_ENABLE, 14, "demand_guard_enable");
point!(DEMAND_LIMIT, 15, "demand_limit");
point!(DEMAND_HYSTERESIS, 16, "demand_hysteresis");

/// EMU 用到的全部逻辑字段，集中在此声明并统一注入 [`collector_core::field::field_registry`]，
/// 不再由各策略各自声明、逐个注册。
pub(crate) const FIELDS: &[FieldSpec] = &[
    field_spec!("bcu_comm_status", "BCU通信状态", Read, "bcu", Id(34)),
    field_spec!("soc", "SOC", Read, "bcu", Id(32)),
    field_spec!("bcu_current", "BCU电流(负充正放)", Read, "bcu", Id(46)),
    field_spec!(
        "pcs_active_power",
        "PCS有功功率设定(正充负放)",
        ReadWrite,
        "pcs",
        Id(2003)
    ),
    // 以下两个点位先预留占位（ID 9999 并不存在），上线前在字段绑定页改绑到真实点位；
    // 未绑定时防逆流会保守地禁止放电，因此默认关闭的保护功能不受影响
    field_spec!(
        "grid_active_power",
        "关口表有功功率(取电正反送负)",
        Read,
        "grid_meter",
        Id(9999)
    ),
    field_spec!(
        "pcs_actual_power",
        "PCS总输出有功功率(负充正放)",
        Read,
        "pcs",
        Id(11)
    ),
];
