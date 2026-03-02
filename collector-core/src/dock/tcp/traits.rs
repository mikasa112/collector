use std::net::SocketAddr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{
    center::{Center, global_center},
    core::point::{DataPoint, DataPoints, Val},
    dev::Identifiable,
    dock::frame::{
        DataPointsFrameExt, FLAG_ACK_REQUIRED, Frame, MsgType, PointDomain, build_data_frame,
        build_dict_frame,
    },
};

use super::error::TcpServerError;

#[async_trait::async_trait]
pub trait FrameHandler: Send + Sync {
    async fn handle(&self, peer: SocketAddr, frame: Frame) -> Result<(), TcpServerError>;
}

struct OwnedDev(String);

impl Identifiable for OwnedDev {
    fn id(&self) -> &str {
        &self.0
    }
}

#[async_trait::async_trait]
pub trait PushFrameProvider: Send + Sync {
    async fn build_connect_dict_frames(
        &self,
        _peer: SocketAddr,
    ) -> Result<Vec<Frame>, TcpServerError> {
        let mut frames = Vec::new();
        for (idx, dev_id) in global_center().dev_ids().into_iter().enumerate() {
            let dev = OwnedDev(dev_id.to_string());
            let points = match global_center().snapshot(&dev) {
                Some(points) if !points.is_empty() => points,
                _ => continue,
            };

            let entries = DataPoints(points).to_frame_dict_entry_specs();
            let frame = build_dict_frame(
                FLAG_ACK_REQUIRED,
                u32::try_from(idx + 1).unwrap_or(u32::MAX),
                now_millis(),
                dev_id.to_string(),
                1,
                &entries,
            )?;
            frames.push(frame);
        }
        Ok(frames)
    }

    async fn build_push_frames(&self, _peer: SocketAddr) -> Result<Vec<Frame>, TcpServerError> {
        let mut frames = Vec::new();
        for dev_id in global_center().dev_ids() {
            let dev = OwnedDev(dev_id.to_string());
            let points = match global_center().snapshot(&dev) {
                Some(points) if !points.is_empty() => points,
                _ => continue,
            };

            let entries = DataPoints(points).to_frame_point_specs();
            let frame = build_data_frame(0, 0, now_millis(), dev_id.to_string(), &entries)?;
            frames.push(frame);
        }
        Ok(frames)
    }
}

#[derive(Default)]
pub struct LoggingFrameHandler;

#[async_trait::async_trait]
impl FrameHandler for LoggingFrameHandler {
    async fn handle(&self, peer: SocketAddr, frame: Frame) -> Result<(), TcpServerError> {
        if frame.header.msg_type == MsgType::Control {
            let dev = OwnedDev(String::from_utf8_lossy(frame.device_id.as_ref()).to_string());
            let entry = parse_control_point(frame.payload.as_ref())?;
            let entries = vec![entry];
            if !entries.is_empty() {
                global_center()
                    .dispatch(&dev, entries)
                    .await
                    .map_err(|e| TcpServerError::HeaderError(e.to_string()))?;
            }
        }

        tracing::info!(
            "recv frame from {}: type={:?}, seq={}, payload_len={}",
            peer,
            frame.header.msg_type,
            frame.header.seq,
            frame.header.payload_len
        );
        Ok(())
    }
}

#[derive(Default)]
pub struct NoopPushFrameProvider;

#[async_trait::async_trait]
impl PushFrameProvider for NoopPushFrameProvider {}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn parse_control_point(payload: &[u8]) -> Result<DataPoint, TcpServerError> {
    if payload.len() < 15 {
        return Err(TcpServerError::HeaderError(
            "control payload too short".to_string(),
        ));
    }
    let mut off = 0usize;
    let _cmd_id = u32::from_be_bytes([
        payload[off],
        payload[off + 1],
        payload[off + 2],
        payload[off + 3],
    ]);
    off += 4;
    let point_id = u32::from_be_bytes([
        payload[off],
        payload[off + 1],
        payload[off + 2],
        payload[off + 3],
    ]);
    off += 4;
    let value_type = payload[off];
    off += 1;
    let value_len = u16::from_be_bytes([payload[off], payload[off + 1]]) as usize;
    off += 2;
    if off + value_len + 4 > payload.len() {
        return Err(TcpServerError::HeaderError(
            "control value/timeout truncated".to_string(),
        ));
    }
    let register_type = (point_id >> 16) as u16;
    if register_type != PointDomain::Yk as u16 && register_type != PointDomain::Yt as u16 {
        return Err(TcpServerError::HeaderError(format!(
            "control point_id is not YK/YT: {}",
            point_id
        )));
    }
    let raw = &payload[off..off + value_len];
    Ok(DataPoint {
        id: point_id,
        name: "",
        value: decode_val(value_type, raw)?,
    })
}

fn decode_val(value_type: u8, raw: &[u8]) -> Result<Val, TcpServerError> {
    match (value_type, raw.len()) {
        (1, 1) => Ok(Val::U8(raw[0])),
        (2, 1) => Ok(Val::I8(raw[0] as i8)),
        (3, 2) => Ok(Val::I16(i16::from_be_bytes([raw[0], raw[1]]))),
        (4, 4) => Ok(Val::I32(i32::from_be_bytes([
            raw[0], raw[1], raw[2], raw[3],
        ]))),
        (5, 2) => Ok(Val::U16(u16::from_be_bytes([raw[0], raw[1]]))),
        (6, 4) => Ok(Val::U32(u32::from_be_bytes([
            raw[0], raw[1], raw[2], raw[3],
        ]))),
        (7, 4) => Ok(Val::F32(f32::from_be_bytes([
            raw[0], raw[1], raw[2], raw[3],
        ]))),
        (8, 1) => Ok(Val::U8(u8::from(raw[0] != 0))),
        (9, _) => Err(TcpServerError::HeaderError(
            "UTF8 string is not supported for downlink".to_string(),
        )),
        _ => Err(TcpServerError::HeaderError(format!(
            "invalid value_type/len: type={}, len={}",
            value_type,
            raw.len()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::frame::{ControlSpec, FLAG_ACK_REQUIRED, ValueType, build_control_frame};

    #[tokio::test]
    async fn noop_push_provider_connect_frames_builds_without_error() {
        let provider = NoopPushFrameProvider;
        let _frames = provider
            .build_connect_dict_frames("127.0.0.1:9000".parse().expect("parse test addr"))
            .await
            .expect("build connect dict frames");
    }

    #[tokio::test]
    async fn connect_dict_frames_should_require_ack() {
        struct TestDev(&'static str);
        impl Identifiable for TestDev {
            fn id(&self) -> &str {
                self.0
            }
        }

        let dev = TestDev("test-dict-ack-dev");
        global_center().ingest(
            &dev,
            vec![DataPoint {
                id: 0x0004_0001,
                name: "p1",
                value: Val::U16(1),
            }],
        );

        let provider = NoopPushFrameProvider;
        let frames = provider
            .build_connect_dict_frames("127.0.0.1:9000".parse().expect("parse test addr"))
            .await
            .expect("build connect dict frames");
        let frame = frames
            .into_iter()
            .find(|f| f.device_id == b"test-dict-ack-dev".as_slice())
            .expect("find target dict frame");
        assert_eq!(frame.header.flags & FLAG_ACK_REQUIRED, FLAG_ACK_REQUIRED);
    }

    #[test]
    fn parse_control_point_extracts_value() {
        let spec = ControlSpec::new(
            101,
            0x0001_0008,
            ValueType::U16,
            12u16.to_be_bytes().to_vec(),
            3000,
        );
        let frame = build_control_frame(0, 1, 1, "dev", &spec).expect("build control frame");
        let got = parse_control_point(frame.payload.as_ref()).expect("parse control point");
        assert_eq!(got.id, 0x0001_0008);
        assert_eq!(got.value, Val::U16(12));
    }

    #[test]
    fn parse_control_point_rejects_non_yk_yt_point() {
        let spec = ControlSpec::new(
            101,
            0x0004_0008,
            ValueType::U16,
            12u16.to_be_bytes().to_vec(),
            3000,
        );
        let frame = build_control_frame(0, 1, 1, "dev", &spec).expect("build control frame");
        let err = parse_control_point(frame.payload.as_ref()).expect_err("must reject yc point");
        assert!(err.to_string().contains("not YK/YT"));
    }
}
