use crate::config::modbus_conf::{ModbusConfig, RegisterType};

pub(super) struct ReadBatch<'a> {
    pub(super) register_type: RegisterType,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) configs: Vec<&'a ModbusConfig>,
}

pub(super) fn register_type_key(rt: RegisterType) -> u8 {
    match rt {
        RegisterType::Coils => 0,
        RegisterType::DiscreteInputs => 1,
        RegisterType::HoldingRegisters => 2,
        RegisterType::InputRegisters => 3,
    }
}

pub(super) fn range_end(start: usize, qty: usize) -> Option<usize> {
    if qty == 0 {
        return None;
    }
    start.checked_add(qty)
}
