use collector_core::center::SharedPointCenter;

mod command;

//需要一个数据结构，能够从SharedPointCenter中获得一些点，通过这些点计算(判断)出当前点位的Val，这是实时的
//同时这个数据结构，需要具备一些
pub struct Emu {
    pub center: SharedPointCenter,
}

impl Emu {
    pub fn new(center: SharedPointCenter) -> Self {
        todo!()
    }
}
