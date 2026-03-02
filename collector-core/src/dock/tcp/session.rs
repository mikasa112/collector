use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use bytes::BytesMut;
use smallvec::SmallVec;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time;

use crate::dock::frame::{
    FLAG_ACK_REQUIRED, Frame, FrameError, Header, MsgType,
    build_ack_frame as frame_build_ack_frame, build_error_frame as frame_build_error_frame,
};

use super::config::TcpServerConfig;
use super::error::TcpServerError;
use super::traits::{FrameHandler, PushFrameProvider};

pub(crate) async fn handle_connection(
    mut socket: TcpStream,
    peer: SocketAddr,
    cfg: TcpServerConfig,
    handler: Arc<dyn FrameHandler>,
    push_provider: Arc<dyn PushFrameProvider>,
) -> Result<(), TcpServerError> {
    let mut pending = BytesMut::with_capacity(8192);
    let mut last_seen = Instant::now();
    let mut heartbeat_ticker = time::interval(cfg.heartbeat_check_interval);
    let mut push_ticker = time::interval(cfg.push_interval);
    let mut dict_ack_ticker = time::interval(cfg.dict_ack_check_interval);
    let mut next_out_seq = 1u32;
    let mut pending_dict_acks: HashMap<u32, PendingAck> = HashMap::new();
    let mut bound_control_device: Option<bytes::Bytes> = None;

    let connect_dict_frames = push_provider.build_connect_dict_frames(peer).await?;
    for mut frame in connect_dict_frames {
        frame.header.seq = alloc_session_seq(&mut next_out_seq);
        let track_ack = frame.header.flags & FLAG_ACK_REQUIRED != 0;
        let tracked_frame = if track_ack { Some(frame.clone()) } else { None };
        send_frame(&mut socket, frame, &cfg).await?;
        if let Some(tracked_frame) = tracked_frame {
            pending_dict_acks.insert(
                tracked_frame.header.seq,
                PendingAck {
                    frame: tracked_frame,
                    sent_at: Instant::now(),
                    retries: 0,
                },
            );
        }
    }

    loop {
        tokio::select! {
            n = socket.read_buf(&mut pending) => {
                let n = n?;
                if n == 0 {
                    return Ok(());
                }
                last_seen = Instant::now();

                loop {
                    if pending.len() < Header::FIXED_LEN {
                        break;
                    }

                    let frame_len = match Frame::peek_frame_len(&pending[..]) {
                        Ok(Some(len)) => len,
                        Ok(None) => break,
                        Err(FrameError::BadMagic(_)) => {
                            let _ = pending.split_to(1);
                            continue;
                        }
                        Err(err) => return Err(err.into()),
                    };

                    if frame_len > cfg.max_frame_len {
                        return Err(TcpServerError::FrameTooLarge(frame_len));
                    }
                    if pending.len() < frame_len {
                        break;
                    }

                    let frame_buf = pending.split_to(frame_len);
                    let frame = Frame::decode(&frame_buf[..])?;
                    last_seen = Instant::now();

                    if let Some(ack_seq) = parse_ack_seq(&frame) {
                        if pending_dict_acks.remove(&ack_seq).is_some() {
                            tracing::info!("dict frame acked from {}: seq={}", peer, ack_seq);
                        }
                    }

                    let responses = process_incoming_frame(
                        peer,
                        handler.as_ref(),
                        frame,
                        &mut next_out_seq,
                        &mut bound_control_device,
                    )
                    .await?;
                    for response in responses {
                        send_frame(&mut socket, response, &cfg).await?;
                    }
                }

                if pending.len() > cfg.max_frame_len {
                    return Err(TcpServerError::FrameTooLarge(pending.len()));
                }
            }
            _ = heartbeat_ticker.tick() => {
                if last_seen.elapsed() > cfg.heartbeat_timeout {
                    return Err(TcpServerError::HeaderError(format!(
                        "heartbeat timeout: no frame from {} for {:?}",
                        peer, cfg.heartbeat_timeout
                    )));
                }
            }
            _ = push_ticker.tick() => {
                let frames = push_provider.build_push_frames(peer).await?;
                for mut frame in frames {
                    frame.header.seq = alloc_session_seq(&mut next_out_seq);
                    send_frame(&mut socket, frame, &cfg).await?;
                }
            }
            _ = dict_ack_ticker.tick() => {
                if pending_dict_acks.is_empty() {
                    continue;
                }

                let now = Instant::now();
                let mut resend_seqs: SmallVec<[u32; 8]> = SmallVec::new();
                for (seq, ack) in &pending_dict_acks {
                    if now.duration_since(ack.sent_at) < cfg.dict_ack_timeout {
                        continue;
                    }
                    if ack.retries >= cfg.dict_ack_max_retries {
                        return Err(TcpServerError::HeaderError(format!(
                            "dict ack timeout from {}: seq={}, retries={}",
                            peer, seq, ack.retries
                        )));
                    }
                    resend_seqs.push(*seq);
                }

                for seq in resend_seqs {
                    if let Some(ack) = pending_dict_acks.get_mut(&seq) {
                        send_frame(&mut socket, ack.frame.clone(), &cfg).await?;
                        ack.sent_at = Instant::now();
                        ack.retries = ack.retries.saturating_add(1);
                        tracing::warn!(
                            "resend dict frame to {}: seq={}, retries={}",
                            peer,
                            seq,
                            ack.retries
                        );
                    }
                }
            }
        }
    }
}

#[derive(Clone)]
struct PendingAck {
    frame: Frame,
    sent_at: Instant,
    retries: u32,
}

fn alloc_session_seq(next_out_seq: &mut u32) -> u32 {
    let out = *next_out_seq;
    *next_out_seq = next_out_seq.wrapping_add(1);
    if *next_out_seq == 0 {
        *next_out_seq = 1;
    }
    out
}

fn parse_ack_seq(frame: &Frame) -> Option<u32> {
    if frame.header.msg_type != MsgType::Ack {
        return None;
    }
    if frame.payload.len() < 4 {
        return Some(frame.header.seq);
    }
    Some(u32::from_be_bytes([
        frame.payload[0],
        frame.payload[1],
        frame.payload[2],
        frame.payload[3],
    ]))
}

async fn send_frame(
    socket: &mut TcpStream,
    mut frame: Frame,
    cfg: &TcpServerConfig,
) -> Result<(), TcpServerError> {
    if cfg.enable_compression {
        frame.maybe_compress_payload(cfg.compress_threshold, cfg.compress_level)?;
    }
    let encoded = frame.encode()?;
    socket.write_all(&encoded).await?;
    Ok(())
}

async fn process_incoming_frame(
    peer: SocketAddr,
    handler: &dyn FrameHandler,
    frame: Frame,
    next_out_seq: &mut u32,
    bound_control_device: &mut Option<bytes::Bytes>,
) -> Result<Vec<Frame>, TcpServerError> {
    if frame.has_unknown_flags() {
        return Ok(vec![build_error_frame(
            alloc_session_seq(next_out_seq),
            1006,
            frame.header.seq,
            frame.device_id.clone(),
            "unknown frame flags",
        )?]);
    }

    match frame.header.msg_type {
        MsgType::Heartbeat => {
            if frame.header.flags & FLAG_ACK_REQUIRED != 0 {
                Ok(vec![build_ack_frame(
                    alloc_session_seq(next_out_seq),
                    frame.header.seq,
                    frame.device_id.clone(),
                    0,
                    "",
                )?])
            } else {
                Ok(Vec::new())
            }
        }
        MsgType::Ack => {
            tracing::debug!("recv ack frame from {} seq={}", peer, frame.header.seq);
            Ok(Vec::new())
        }
        MsgType::Error => {
            tracing::warn!("recv error frame from {} seq={}", peer, frame.header.seq);
            Ok(Vec::new())
        }
        _ => handle_business_frame(peer, handler, frame, next_out_seq, bound_control_device).await,
    }
}

async fn handle_business_frame(
    peer: SocketAddr,
    handler: &dyn FrameHandler,
    frame: Frame,
    next_out_seq: &mut u32,
    bound_control_device: &mut Option<bytes::Bytes>,
) -> Result<Vec<Frame>, TcpServerError> {
    let ack_required =
        frame.header.flags & FLAG_ACK_REQUIRED != 0 || frame.header.msg_type == MsgType::Control;
    let seq = frame.header.seq;
    let device_id = frame.device_id.clone();
    let msg_type = frame.header.msg_type;
    if msg_type == MsgType::Control {
        match bound_control_device {
            None => {
                *bound_control_device = Some(device_id.clone());
            }
            Some(bound) if *bound != device_id => {
                tracing::warn!(
                    "reject control frame from {}: session bound device={}, incoming device={}",
                    peer,
                    String::from_utf8_lossy(bound.as_ref()),
                    String::from_utf8_lossy(device_id.as_ref())
                );
                if ack_required {
                    return Ok(vec![build_ack_frame(
                        alloc_session_seq(next_out_seq),
                        seq,
                        device_id,
                        1006,
                        "control device_id mismatch in session",
                    )?]);
                }
                return Ok(vec![build_error_frame(
                    alloc_session_seq(next_out_seq),
                    1006,
                    seq,
                    device_id,
                    "control device_id mismatch in session",
                )?]);
            }
            Some(_) => {}
        }
    }

    match handler.handle(peer, frame).await {
        Ok(()) => {
            if ack_required {
                return Ok(vec![build_ack_frame(
                    alloc_session_seq(next_out_seq),
                    seq,
                    device_id,
                    0,
                    "",
                )?]);
            }
            Ok(Vec::new())
        }
        Err(err) => {
            let err_text = err.to_string();
            tracing::warn!(
                "handle business frame failed from {}: type={:?}, seq={}, err={}",
                peer,
                msg_type,
                seq,
                err_text
            );
            if ack_required {
                return Ok(vec![build_ack_frame(
                    alloc_session_seq(next_out_seq),
                    seq,
                    device_id,
                    1007,
                    &err_text,
                )?]);
            }
            Ok(vec![build_error_frame(
                alloc_session_seq(next_out_seq),
                1007,
                seq,
                device_id,
                &err_text,
            )?])
        }
    }
}

fn build_ack_frame(
    seq: u32,
    ack_seq: u32,
    device_id: bytes::Bytes,
    code: u16,
    msg: &str,
) -> Result<Frame, TcpServerError> {
    let msg_bytes = msg.as_bytes();
    let msg_len = msg_bytes.len().min(u8::MAX as usize);
    frame_build_ack_frame(
        seq,
        now_millis(),
        device_id,
        ack_seq,
        code,
        &msg_bytes[..msg_len],
    )
    .map_err(Into::into)
}

fn build_error_frame(
    seq: u32,
    err_code: u16,
    ref_seq: u32,
    device_id: bytes::Bytes,
    msg: &str,
) -> Result<Frame, TcpServerError> {
    let msg_bytes = msg.as_bytes();
    let msg_len = msg_bytes.len().min(u8::MAX as usize);
    frame_build_error_frame(
        seq,
        ref_seq,
        now_millis(),
        device_id,
        err_code,
        &msg_bytes[..msg_len],
    )
    .map_err(Into::into)
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::dock::frame::FLAG_IS_RESPONSE;

    use super::*;

    struct OkHandler;

    #[async_trait::async_trait]
    impl FrameHandler for OkHandler {
        async fn handle(&self, _peer: SocketAddr, _frame: Frame) -> Result<(), TcpServerError> {
            Ok(())
        }
    }

    struct ErrHandler;

    #[async_trait::async_trait]
    impl FrameHandler for ErrHandler {
        async fn handle(&self, _peer: SocketAddr, _frame: Frame) -> Result<(), TcpServerError> {
            Err(TcpServerError::HeaderError("handler error".to_string()))
        }
    }

    fn parse_error_payload(payload: &[u8]) -> (u16, u32, String) {
        assert!(payload.len() >= 7, "error payload too short");
        let err_code = u16::from_be_bytes([payload[0], payload[1]]);
        let ref_seq = u32::from_be_bytes([payload[2], payload[3], payload[4], payload[5]]);
        let msg_len = payload[6] as usize;
        assert!(
            payload.len() >= 7 + msg_len,
            "error payload msg_len out of range"
        );
        let msg = String::from_utf8_lossy(&payload[7..7 + msg_len]).to_string();
        (err_code, ref_seq, msg)
    }

    fn parse_ack_payload(payload: &[u8]) -> (u32, u16, String) {
        assert!(payload.len() >= 7, "ack payload too short");
        let ack_seq = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let code = u16::from_be_bytes([payload[4], payload[5]]);
        let msg_len = payload[6] as usize;
        assert!(
            payload.len() >= 7 + msg_len,
            "ack payload msg_len out of range"
        );
        let msg = String::from_utf8_lossy(&payload[7..7 + msg_len]).to_string();
        (ack_seq, code, msg)
    }

    #[test]
    fn parse_ack_seq_reads_payload_ack_seq() {
        let frame = frame_build_ack_frame(11, now_millis(), b"dev-ack".to_vec(), 321, 0, b"")
            .expect("build ack frame");
        assert_eq!(parse_ack_seq(&frame), Some(321));
    }

    #[tokio::test]
    async fn business_frame_with_ack_required_returns_ack_frame() {
        let req = Frame::new(
            MsgType::Data,
            FLAG_ACK_REQUIRED,
            101,
            now_millis(),
            b"dev-1".to_vec(),
            b"abc".to_vec(),
        )
        .expect("build request frame");
        let handler = Arc::new(OkHandler);
        let mut next_out_seq = 1u32;
        let mut bound_control_device: Option<bytes::Bytes> = None;
        let responses = process_incoming_frame(
            "127.0.0.1:9000".parse().expect("parse test addr"),
            handler.as_ref(),
            req,
            &mut next_out_seq,
            &mut bound_control_device,
        )
        .await
        .expect("process frame");

        assert_eq!(responses.len(), 1);
        let resp = &responses[0];
        assert_eq!(resp.header.msg_type, MsgType::Ack);
        assert_eq!(resp.header.flags, FLAG_IS_RESPONSE);
        assert_eq!(resp.header.seq, 1);
        assert_eq!(resp.device_id, b"dev-1".to_vec());
        let (ack_seq, code, msg) = parse_ack_payload(&resp.payload);
        assert_eq!(ack_seq, 101);
        assert_eq!(code, 0);
        assert!(msg.is_empty());
    }

    #[tokio::test]
    async fn business_handler_error_returns_ack_frame_with_error_code() {
        let req = Frame::new(
            MsgType::Control,
            0,
            202,
            now_millis(),
            b"dev-err".to_vec(),
            b"payload".to_vec(),
        )
        .expect("build request frame");
        let handler = Arc::new(ErrHandler);
        let mut next_out_seq = 1u32;
        let mut bound_control_device: Option<bytes::Bytes> = None;
        let responses = process_incoming_frame(
            "127.0.0.1:9000".parse().expect("parse test addr"),
            handler.as_ref(),
            req,
            &mut next_out_seq,
            &mut bound_control_device,
        )
        .await
        .expect("process frame");

        assert_eq!(responses.len(), 1);
        let resp = &responses[0];
        assert_eq!(resp.header.msg_type, MsgType::Ack);
        assert_eq!(resp.header.flags, FLAG_IS_RESPONSE);
        assert_eq!(resp.header.seq, 1);
        assert_eq!(resp.device_id, b"dev-err".to_vec());

        let (ack_seq, code, msg) = parse_ack_payload(&resp.payload);
        assert_eq!(ack_seq, 202);
        assert_eq!(code, 1007);
        assert!(msg.contains("handler error"));
    }

    #[tokio::test]
    async fn heartbeat_with_ack_required_returns_ack_frame() {
        let heartbeat = Frame::new(
            MsgType::Heartbeat,
            FLAG_ACK_REQUIRED,
            303,
            now_millis(),
            b"dev-hb".to_vec(),
            Vec::new(),
        )
        .expect("build heartbeat frame");
        let handler = Arc::new(OkHandler);
        let mut next_out_seq = 1u32;
        let mut bound_control_device: Option<bytes::Bytes> = None;
        let responses = process_incoming_frame(
            "127.0.0.1:9000".parse().expect("parse test addr"),
            handler.as_ref(),
            heartbeat,
            &mut next_out_seq,
            &mut bound_control_device,
        )
        .await
        .expect("process frame");

        assert_eq!(responses.len(), 1);
        let resp = &responses[0];
        assert_eq!(resp.header.msg_type, MsgType::Ack);
        assert_eq!(resp.header.seq, 1);
        assert_eq!(resp.device_id, b"dev-hb".to_vec());
        let (ack_seq, code, _msg) = parse_ack_payload(&resp.payload);
        assert_eq!(ack_seq, 303);
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn frame_with_unknown_flags_returns_invalid_payload_error() {
        let req = Frame::new(
            MsgType::Data,
            FLAG_ACK_REQUIRED | (1 << 12),
            404,
            now_millis(),
            b"dev-flag".to_vec(),
            b"payload".to_vec(),
        )
        .expect("build request frame");
        let handler = Arc::new(OkHandler);
        let mut next_out_seq = 1u32;
        let mut bound_control_device: Option<bytes::Bytes> = None;
        let responses = process_incoming_frame(
            "127.0.0.1:9000".parse().expect("parse test addr"),
            handler.as_ref(),
            req,
            &mut next_out_seq,
            &mut bound_control_device,
        )
        .await
        .expect("process frame");

        assert_eq!(responses.len(), 1);
        let resp = &responses[0];
        assert_eq!(resp.header.msg_type, MsgType::Error);
        assert_eq!(resp.header.seq, 1);
        let (err_code, ref_seq, msg) = parse_error_payload(&resp.payload);
        assert_eq!(err_code, 1006);
        assert_eq!(ref_seq, 404);
        assert!(msg.contains("unknown frame flags"));
    }

    #[tokio::test]
    async fn ack_frame_produces_no_response() {
        let req = Frame::new(
            MsgType::Ack,
            0,
            505,
            now_millis(),
            b"dev-ack".to_vec(),
            vec![0, 0, 1, 2, 0, 0, 0],
        )
        .expect("build ack frame");

        let handler = Arc::new(OkHandler);
        let mut next_out_seq = 1u32;
        let mut bound_control_device: Option<bytes::Bytes> = None;
        let responses = process_incoming_frame(
            "127.0.0.1:9000".parse().expect("parse test addr"),
            handler.as_ref(),
            req,
            &mut next_out_seq,
            &mut bound_control_device,
        )
        .await
        .expect("process frame");
        assert!(responses.is_empty());
    }

    #[tokio::test]
    async fn control_frame_without_ack_required_still_returns_ack() {
        let req = Frame::new(
            MsgType::Control,
            0,
            606,
            now_millis(),
            b"dev-ctl".to_vec(),
            b"payload".to_vec(),
        )
        .expect("build control frame");
        let handler = Arc::new(OkHandler);
        let mut next_out_seq = 1u32;
        let mut bound_control_device: Option<bytes::Bytes> = None;
        let responses = process_incoming_frame(
            "127.0.0.1:9000".parse().expect("parse test addr"),
            handler.as_ref(),
            req,
            &mut next_out_seq,
            &mut bound_control_device,
        )
        .await
        .expect("process frame");
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].header.msg_type, MsgType::Ack);
        assert_eq!(responses[0].header.seq, 1);
        let (ack_seq, code, _msg) = parse_ack_payload(&responses[0].payload);
        assert_eq!(ack_seq, 606);
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn control_frame_with_different_device_in_same_session_is_rejected() {
        let handler = Arc::new(OkHandler);
        let mut next_out_seq = 1u32;
        let mut bound_control_device: Option<bytes::Bytes> = None;

        let first = Frame::new(
            MsgType::Control,
            0,
            701,
            now_millis(),
            b"dev-a".to_vec(),
            b"payload".to_vec(),
        )
        .expect("build first control frame");
        let second = Frame::new(
            MsgType::Control,
            0,
            702,
            now_millis(),
            b"dev-b".to_vec(),
            b"payload".to_vec(),
        )
        .expect("build second control frame");

        let first_resp = process_incoming_frame(
            "127.0.0.1:9000".parse().expect("parse test addr"),
            handler.as_ref(),
            first,
            &mut next_out_seq,
            &mut bound_control_device,
        )
        .await
        .expect("process first frame");
        assert_eq!(first_resp.len(), 1);
        let (ack_seq1, code1, _msg1) = parse_ack_payload(&first_resp[0].payload);
        assert_eq!(ack_seq1, 701);
        assert_eq!(code1, 0);

        let second_resp = process_incoming_frame(
            "127.0.0.1:9000".parse().expect("parse test addr"),
            handler.as_ref(),
            second,
            &mut next_out_seq,
            &mut bound_control_device,
        )
        .await
        .expect("process second frame");
        assert_eq!(second_resp.len(), 1);
        assert_eq!(second_resp[0].header.msg_type, MsgType::Ack);
        let (ack_seq2, code2, msg2) = parse_ack_payload(&second_resp[0].payload);
        assert_eq!(ack_seq2, 702);
        assert_eq!(code2, 1006);
        assert!(msg2.contains("mismatch"));
    }
}
