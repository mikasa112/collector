use serde::Serialize;

/// t_project_info 中使用的键
pub const KEY_NAME: &str = "name";
pub const KEY_TITLE: &str = "title";
pub const KEY_VERSION: &str = "version";
pub const KEY_RATED_POWER_KW: &str = "rated_power_kw";
pub const KEY_RATED_ENERGY_KWH: &str = "rated_energy_kwh";

#[derive(Debug, Serialize)]
pub struct ProjectInfoView {
    /// 项目名称
    pub name: Option<String>,
    /// 项目标题
    pub title: Option<String>,
    /// 项目版本
    pub version: Option<String>,
    /// 额定功率(kW)
    pub rated_power_kw: Option<f64>,
    /// 额定能量(kWh)
    pub rated_energy_kwh: Option<f64>,
    /// 采集程序软件版本（编译期确定，只读）
    pub software_version: &'static str,
    /// 图标地址，未上传时为 null；带修改时间戳用于前端缓存刷新
    pub logo_url: Option<String>,
}
