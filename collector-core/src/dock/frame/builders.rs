#![allow(dead_code)]

use bytes::{Bytes, BytesMut};

use super::{FLAG_IS_RESPONSE, Frame, FrameError, MsgType, PointDomain, ValueType};

#[derive(Debug, Clone)]
pub struct DictEntrySpec {
    pub point_id: u32,
    pub name: Bytes,
    pub unit: Bytes,
    pub value_type: ValueType,
}

impl DictEntrySpec {
    pub fn new(
        point_id: u32,
        value_type: ValueType,
        name: impl Into<Bytes>,
        unit: impl Into<Bytes>,
    ) -> Self {
        Self {
            point_id,
            name: name.into(),
            unit: unit.into(),
            value_type,
        }
    }
}

#[derive(Debug, Clone)]
pub struct PointSpec {
    pub point_id: u32,
    pub domain: PointDomain,
    pub value_type: ValueType,
    pub value: Bytes,
}

impl PointSpec {
    pub fn new(
        point_id: u32,
        domain: PointDomain,
        value_type: ValueType,
        value: impl Into<Bytes>,
    ) -> Self {
        Self {
            point_id,
            domain,
            value_type,
            value: value.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControlSpec {
    pub cmd_id: u32,
    pub point_id: u32,
    pub value_type: ValueType,
    pub value: Bytes,
    pub timeout_ms: u32,
}

impl ControlSpec {
    pub fn new(
        cmd_id: u32,
        point_id: u32,
        value_type: ValueType,
        value: impl Into<Bytes>,
        timeout_ms: u32,
    ) -> Self {
        Self {
            cmd_id,
            point_id,
            value_type,
            value: value.into(),
            timeout_ms,
        }
    }
}

pub fn build_dict_frame(
    flags: u16,
    seq: u32,
    timestamp_ms: u64,
    device_id: impl Into<Bytes>,
    dict_version: u32,
    entries: &[DictEntrySpec],
) -> Result<Frame, FrameError> {
    let entry_count = u16::try_from(entries.len()).map_err(|_| FrameError::FieldTooLarge)?;
    let mut payload = BytesMut::with_capacity(6 + entries.len() * 16);
    payload.extend_from_slice(&dict_version.to_be_bytes());
    payload.extend_from_slice(&entry_count.to_be_bytes());

    for entry in entries {
        let name_len = u16::try_from(entry.name.len()).map_err(|_| FrameError::FieldTooLarge)?;
        let unit_len = u8::try_from(entry.unit.len()).map_err(|_| FrameError::FieldTooLarge)?;

        payload.extend_from_slice(&entry.point_id.to_be_bytes());
        payload.extend_from_slice(&name_len.to_be_bytes());
        payload.extend_from_slice(entry.name.as_ref());
        payload.extend_from_slice(&[unit_len]);
        payload.extend_from_slice(entry.unit.as_ref());
        payload.extend_from_slice(&[entry.value_type as u8]);
    }

    Frame::new(
        MsgType::Dict,
        flags,
        seq,
        timestamp_ms,
        device_id,
        payload.freeze(),
    )
}

pub fn build_data_frame(
    flags: u16,
    seq: u32,
    timestamp_ms: u64,
    device_id: impl Into<Bytes>,
    points: &[PointSpec],
) -> Result<Frame, FrameError> {
    build_points_frame(MsgType::Data, flags, seq, timestamp_ms, device_id, points)
}

pub fn build_control_frame(
    flags: u16,
    seq: u32,
    timestamp_ms: u64,
    device_id: impl Into<Bytes>,
    spec: &ControlSpec,
) -> Result<Frame, FrameError> {
    build_control_frame_inner(MsgType::Control, flags, seq, timestamp_ms, device_id, spec)
}

pub fn build_heartbeat_frame(
    flags: u16,
    seq: u32,
    timestamp_ms: u64,
    device_id: impl Into<Bytes>,
    status: u8,
    uptime_s: u32,
) -> Result<Frame, FrameError> {
    let mut payload = BytesMut::with_capacity(5);
    payload.extend_from_slice(&[status]);
    payload.extend_from_slice(&uptime_s.to_be_bytes());
    Frame::new(
        MsgType::Heartbeat,
        flags,
        seq,
        timestamp_ms,
        device_id,
        payload.freeze(),
    )
}

pub fn build_ack_frame(
    seq: u32,
    timestamp_ms: u64,
    device_id: impl Into<Bytes>,
    ack_seq: u32,
    code: u16,
    msg: &[u8],
) -> Result<Frame, FrameError> {
    let msg_len = u8::try_from(msg.len()).map_err(|_| FrameError::FieldTooLarge)?;
    let mut payload = BytesMut::with_capacity(7 + msg.len());
    payload.extend_from_slice(&ack_seq.to_be_bytes());
    payload.extend_from_slice(&code.to_be_bytes());
    payload.extend_from_slice(&[msg_len]);
    payload.extend_from_slice(msg);
    Frame::new(
        MsgType::Ack,
        FLAG_IS_RESPONSE,
        seq,
        timestamp_ms,
        device_id,
        payload.freeze(),
    )
}

pub fn build_error_frame(
    seq: u32,
    ref_seq: u32,
    timestamp_ms: u64,
    device_id: impl Into<Bytes>,
    err_code: u16,
    msg: &[u8],
) -> Result<Frame, FrameError> {
    let msg_len = u8::try_from(msg.len()).map_err(|_| FrameError::FieldTooLarge)?;
    let mut payload = BytesMut::with_capacity(7 + msg.len());
    payload.extend_from_slice(&err_code.to_be_bytes());
    payload.extend_from_slice(&ref_seq.to_be_bytes());
    payload.extend_from_slice(&[msg_len]);
    payload.extend_from_slice(msg);
    Frame::new(
        MsgType::Error,
        FLAG_IS_RESPONSE,
        seq,
        timestamp_ms,
        device_id,
        payload.freeze(),
    )
}

fn build_points_frame(
    msg_type: MsgType,
    flags: u16,
    seq: u32,
    timestamp_ms: u64,
    device_id: impl Into<Bytes>,
    points: &[PointSpec],
) -> Result<Frame, FrameError> {
    let point_count = u16::try_from(points.len()).map_err(|_| FrameError::FieldTooLarge)?;
    let mut payload = BytesMut::with_capacity(2 + points.len() * 13);
    payload.extend_from_slice(&point_count.to_be_bytes());

    for point in points {
        let value_len = u16::try_from(point.value.len()).map_err(|_| FrameError::FieldTooLarge)?;
        payload.extend_from_slice(&point.point_id.to_be_bytes());
        payload.extend_from_slice(&[point.domain as u8]);
        payload.extend_from_slice(&[point.value_type as u8]);
        payload.extend_from_slice(&value_len.to_be_bytes());
        payload.extend_from_slice(point.value.as_ref());
    }

    Frame::new(
        msg_type,
        flags,
        seq,
        timestamp_ms,
        device_id,
        payload.freeze(),
    )
}

fn build_control_frame_inner(
    msg_type: MsgType,
    flags: u16,
    seq: u32,
    timestamp_ms: u64,
    device_id: impl Into<Bytes>,
    spec: &ControlSpec,
) -> Result<Frame, FrameError> {
    let value_len = u16::try_from(spec.value.len()).map_err(|_| FrameError::FieldTooLarge)?;
    let mut payload = BytesMut::with_capacity(15 + spec.value.len());
    payload.extend_from_slice(&spec.cmd_id.to_be_bytes());
    payload.extend_from_slice(&spec.point_id.to_be_bytes());
    payload.extend_from_slice(&[spec.value_type as u8]);
    payload.extend_from_slice(&value_len.to_be_bytes());
    payload.extend_from_slice(spec.value.as_ref());
    payload.extend_from_slice(&spec.timeout_ms.to_be_bytes());

    Frame::new(
        msg_type,
        flags,
        seq,
        timestamp_ms,
        device_id,
        payload.freeze(),
    )
}
