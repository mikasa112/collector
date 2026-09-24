use std::collections::HashMap;

use mlua::RegistryKey;

/// 同 VM 内插件钩子注册表。区别于 `EventBus`（单向广播，无返回值）：
/// `on` 支持多个扩展处理器（仅产生副作用），`override_` 只保留最后一次注册的替换处理器（有返回值）。
pub struct HookRegistry {
    pub on: HashMap<String, Vec<RegistryKey>>,
    pub override_: HashMap<String, RegistryKey>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            on: HashMap::new(),
            override_: HashMap::new(),
        }
    }
}
