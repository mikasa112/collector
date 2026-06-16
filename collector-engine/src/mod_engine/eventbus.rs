use std::collections::HashMap;

use mlua::RegistryKey;

pub struct EventBus {
    pub handlers: HashMap<String, Vec<RegistryKey>>,
}

impl EventBus {
    pub fn new() -> Self {
        Self {
            handlers: HashMap::new(),
        }
    }
}
