use bytes::Bytes;

pub const MAGIC: u16 = 0xCC01;
pub const VERSION: u8 = 0x01;

pub const FLAG_ACK_REQUIRED: u16 = 1 << 0;
pub const FLAG_IS_RESPONSE: u16 = 1 << 1;
pub const FLAG_IS_FRAGMENT: u16 = 1 << 2;
pub const FLAG_IS_COMPRESSED: u16 = 1 << 3;
pub const KNOWN_FLAGS_MASK: u16 =
    FLAG_ACK_REQUIRED | FLAG_IS_RESPONSE | FLAG_IS_FRAGMENT | FLAG_IS_COMPRESSED;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MsgType {
    Data = 1,
    Control = 2,
    Dict = 5,
    Heartbeat = 6,
    Ack = 7,
    Error = 0xFF,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PointDomain {
    Unknown = 0,
    Yk = 1,
    Yx = 2,
    Yt = 3,
    Yc = 4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ValueType {
    U8 = 1,
    I8 = 2,
    I16 = 3,
    I32 = 4,
    U16 = 5,
    U32 = 6,
    F32 = 7,
    Bool = 8,
    Utf8String = 9,
}

#[derive(Debug, Clone)]
pub struct Header {
    pub magic: u16,
    pub version: u8,
    pub msg_type: MsgType,
    pub flags: u16,
    pub header_len: u16,
    pub payload_len: u32,
    pub seq: u32,
    pub timestamp_ms: u64,
    pub device_id_len: u16,
}

impl Header {
    pub const FIXED_LEN: usize = 26;
}

#[derive(Debug, Clone)]
pub struct Dict {
    pub dict_version: u32,
    pub entry_count: u16,
    pub entries: Vec<DictEntry>,
}

#[derive(Debug, Clone)]
pub struct DictEntry {
    pub point_id: u32,
    pub name_len: u16,
    pub name: Bytes,
    pub unit_len: u8,
    pub unit: Bytes,
    pub value_type: ValueType,
}

#[derive(Debug, Clone)]
pub struct Point {
    pub id: u32,
    pub domain: PointDomain,
    pub value_type: ValueType,
    pub len: u16,
    pub data: Bytes,
}

#[derive(Debug, Clone)]
pub struct Frame {
    pub header: Header,
    pub device_id: Bytes,
    pub payload: Bytes,
    pub crc: u32,
}
