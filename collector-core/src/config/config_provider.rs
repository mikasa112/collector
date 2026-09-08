use std::sync::OnceLock;

use super::program_conf::Program;

static CONFIG_PROVIDER: OnceLock<ConfigProvider> = OnceLock::new();

/// 使用加载到的功能特性配置初始化全局配置提供者，程序启动时调用一次
pub fn init_config_provider(program: Program) {
    CONFIG_PROVIDER
        .set(ConfigProvider::build(program))
        .expect("config_provider 已初始化，不能重复初始化");
}

/// 全局访问功能特性配置，需先调用 init_config_provider 完成初始化
pub fn config_provider() -> &'static ConfigProvider {
    CONFIG_PROVIDER
        .get()
        .expect("config_provider 尚未初始化，请先调用 init_config_provider")
}

#[derive(Debug)]
pub struct ConfigProvider {
    program: Program,
}

impl ConfigProvider {
    fn build(program: Program) -> Self {
        ConfigProvider { program }
    }

    pub fn program(&self) -> &Program {
        &self.program
    }
}
