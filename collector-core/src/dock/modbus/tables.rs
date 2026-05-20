use std::collections::HashMap;

use crate::config::modbus_conf::RegisterType;

pub struct RegisterTable {
    coils: HashMap<u16, bool>,
    discrete_inputs: HashMap<u16, bool>,
    holding_registers: HashMap<u16, u16>,
    input_registers: HashMap<u16, u16>,
}

impl RegisterTable {
    pub fn new() -> Self {
        Self {
            coils: HashMap::new(),
            discrete_inputs: HashMap::new(),
            holding_registers: HashMap::new(),
            input_registers: HashMap::new(),
        }
    }

    pub fn write_bool(&mut self, reg_type: RegisterType, addr: u16, val: bool) {
        match reg_type {
            RegisterType::Coils => {
                self.coils.insert(addr, val);
            }
            RegisterType::DiscreteInputs => {
                self.discrete_inputs.insert(addr, val);
            }
            _ => {}
        }
    }

    pub fn write_u16(&mut self, reg_type: RegisterType, addr: u16, val: u16) {
        match reg_type {
            RegisterType::HoldingRegisters => {
                self.holding_registers.insert(addr, val);
            }
            RegisterType::InputRegisters => {
                self.input_registers.insert(addr, val);
            }
            _ => {}
        }
    }

    pub fn write_u16_pair(&mut self, reg_type: RegisterType, addr: u16, vals: [u16; 2]) {
        self.write_u16(reg_type, addr, vals[0]);
        if let Some(next) = addr.checked_add(1) {
            self.write_u16(reg_type, next, vals[1]);
        }
    }

    pub fn read_coils(&self, addr: u16, cnt: u16) -> Vec<bool> {
        (addr..addr.saturating_add(cnt))
            .map(|a| self.coils.get(&a).copied().unwrap_or(false))
            .collect()
    }

    pub fn read_discrete_inputs(&self, addr: u16, cnt: u16) -> Vec<bool> {
        (addr..addr.saturating_add(cnt))
            .map(|a| self.discrete_inputs.get(&a).copied().unwrap_or(false))
            .collect()
    }

    pub fn read_holding_registers(&self, addr: u16, cnt: u16) -> Vec<u16> {
        (addr..addr.saturating_add(cnt))
            .map(|a| self.holding_registers.get(&a).copied().unwrap_or(0))
            .collect()
    }

    pub fn read_input_registers(&self, addr: u16, cnt: u16) -> Vec<u16> {
        (addr..addr.saturating_add(cnt))
            .map(|a| self.input_registers.get(&a).copied().unwrap_or(0))
            .collect()
    }
}
