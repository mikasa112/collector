#![allow(dead_code)]

pub(crate) const MAGIC: u16 = 0xCC01;
pub(crate) const VERSION: u8 = 0x01;

pub(crate) const FLAG_ACK_REQUIRED: u16 = 1 << 0;
pub(crate) const FLAG_IS_RESPONSE: u16 = 1 << 1;
pub(crate) const FLAG_IS_COMPRESSED: u16 = 1 << 3;
pub(crate) const KNOWN_FLAGS_MASK: u16 = FLAG_ACK_REQUIRED | FLAG_IS_RESPONSE | FLAG_IS_COMPRESSED;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum MsgType {
    Data = 1,
    Control = 2,
    Dict = 5,
    Heartbeat = 6,
    Ack = 7,
    Error = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum Domain {
    Unknown = 0,
    Yx = 1,
    Yc = 2,
    Yk = 3,
    Yt = 4,
}

#[derive(Debug)]
pub(crate) struct Header {
    pub(crate) magic: u16,
    pub(crate) version: u8,
    pub(crate) msg_type: MsgType,
}
