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
