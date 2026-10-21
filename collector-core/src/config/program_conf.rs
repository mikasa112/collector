use serde::Deserialize;

/// 功能特性配置，对应 config.json 中的 `program` 字段
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct Program {
    pub emu: EmuProgram,
    pub http: HttpProgram,
    #[serde(rename = "mod")]
    pub mod_: ModProgram,
    pub north_modbus: NorthModbusProgram,
    pub north_iec104: NorthIec104Program,
    pub coordinator: CoordinatorProgram,
    pub eg25_gl: Eg25Gl,
    pub mqtt: MqttProgram,
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct EmuProgram {
    pub enable: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct HttpProgram {
    pub enable: bool,
    pub ip: String,
    pub port: u16,
}

impl Default for HttpProgram {
    fn default() -> Self {
        Self {
            enable: true,
            ip: "0.0.0.0".to_string(),
            port: 9091,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ModProgram {
    pub enable: bool,
    pub path: String,
}

impl Default for ModProgram {
    fn default() -> Self {
        Self {
            enable: true,
            path: "lua_scripts".to_string(),
        }
    }
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct NorthModbusProgram {
    pub enable: bool,
    pub north_modbus_host: String,
    pub north_modbus_port: u16,
    pub north_modbus_conf: String,
}

/// 北向 IEC 104 子站，点表见 `docs/iec104_server_config.md`
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct NorthIec104Program {
    pub enable: bool,
    pub host: String,
    pub port: u16,
    /// 点表 xlsx 路径
    pub conf: String,
    /// 最大同时连接数，0 为库默认
    pub max_connections: i32,
    /// 多主站同时在线时各自独立收到突发上送
    pub multi_master: bool,
    /// 两步命令从选择到执行的最长间隔
    pub select_timeout_ms: u64,
    /// 为 true 时执行命令必须先有选择
    pub require_select: bool,
}

impl Default for NorthIec104Program {
    fn default() -> Self {
        Self {
            enable: false,
            host: "0.0.0.0".to_string(),
            port: 2404,
            conf: String::new(),
            max_connections: 0,
            multi_master: false,
            select_timeout_ms: 30_000,
            require_select: false,
        }
    }
}

/// 协调控制器（AGC/AVC）：把多台下级控制器合并成一个，北向只面对合并后的虚拟设备。
/// 说明见 `docs/coordinator.md`。
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct CoordinatorProgram {
    pub enable: bool,
    /// 虚拟设备 id，北向点表的 `source` 填它
    pub id: String,
    /// 聚合、分配的周期
    pub tick_ms: u64,
    /// 远方+投入状态下，北向连接全部断开超过该时间后，总设定值清零；0 表示不启用
    pub upstream_timeout_ms: u64,
    /// 有功分配死区(kW)：新设定值与上次下发值之差小于它不重复下发
    pub p_deadband: f64,
    /// 无功分配死区(kvar)
    pub q_deadband: f64,
    pub members: Vec<CoordinatorMember>,
}

impl Default for CoordinatorProgram {
    fn default() -> Self {
        Self {
            enable: false,
            id: "agc".to_string(),
            tick_ms: 500,
            upstream_timeout_ms: 60_000,
            p_deadband: 1.0,
            q_deadband: 1.0,
            members: Vec::new(),
        }
    }
}

/// 一台下级控制器。各点位都是该设备在 DataCenter 里的点 id，
/// 设定值、实测值、限值采用同一符号约定（与北向一致）。
#[derive(Deserialize, Debug, Clone)]
pub struct CoordinatorMember {
    /// DataCenter 里的设备 id
    pub dev: String,
    /// 额定视在容量(kVA)：既是分配权重，也用于 P²+Q²≤S² 约束
    pub rated_kva: f64,
    pub p_set: u32,
    pub q_set: u32,
    pub p_actual: u32,
    pub q_actual: u32,
    /// 有功上限、下限（由下级上送）
    pub p_upper: u32,
    pub p_lower: u32,
    /// 无功上限、下限（由下级上送）
    pub q_upper: u32,
    pub q_lower: u32,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct Eg25Gl {
    pub enable: bool,
}

impl Default for Eg25Gl {
    fn default() -> Self {
        Self { enable: true }
    }
}

#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct MqttProgram {
    pub enable: bool,
    pub mqtt_host: String,
    pub mqtt_port: u16,
    pub mqtt_username: String,
    pub mqtt_password: String,
    pub mqtt_yt: String,
    pub mqtt_yk: String,
}
