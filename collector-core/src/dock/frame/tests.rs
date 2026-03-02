use super::*;

fn sample_frame() -> Frame {
    Frame::new(
        MsgType::Data,
        FLAG_ACK_REQUIRED,
        42,
        1_738_000_000_000,
        b"dev-01".to_vec(),
        vec![1, 2, 3, 4, 5],
    )
    .expect("build sample frame")
}

#[test]
fn encode_decode_roundtrip() {
    let frame = sample_frame();
    let encoded = frame.encode().expect("encode");
    let decoded = Frame::decode(&encoded).expect("decode");

    assert_eq!(decoded.header.magic, MAGIC);
    assert_eq!(decoded.header.version, VERSION);
    assert_eq!(decoded.header.msg_type, MsgType::Data);
    assert_eq!(decoded.header.flags, FLAG_ACK_REQUIRED);
    assert_eq!(decoded.header.seq, 42);
    assert_eq!(decoded.header.timestamp_ms, 1_738_000_000_000);
    assert_eq!(decoded.device_id, b"dev-01".to_vec());
    assert_eq!(decoded.payload, vec![1, 2, 3, 4, 5]);
}

#[test]
fn peek_frame_len_works() {
    let frame = sample_frame();
    let encoded = frame.encode().expect("encode");
    let len = Frame::peek_frame_len(&encoded).expect("peek");
    assert_eq!(len, Some(encoded.len()));
}

#[test]
fn peek_frame_len_returns_none_when_not_enough_header() {
    let frame = sample_frame();
    let encoded = frame.encode().expect("encode");
    let short = &encoded[..Header::FIXED_LEN - 1];
    let len = Frame::peek_frame_len(short).expect("peek");
    assert_eq!(len, None);
}

#[test]
fn decode_rejects_bad_magic() {
    let frame = sample_frame();
    let mut encoded = frame.encode().expect("encode");
    encoded[0] = 0x00;
    encoded[1] = 0x00;

    let err = Frame::decode(&encoded).expect_err("must reject bad magic");
    assert!(matches!(err, FrameError::BadMagic(_)));
}

#[test]
fn decode_rejects_bad_crc() {
    let frame = sample_frame();
    let mut encoded = frame.encode().expect("encode");
    let n = encoded.len();
    encoded[n - 1] ^= 0xFF;

    let err = Frame::decode(&encoded).expect_err("must reject bad crc");
    assert!(matches!(err, FrameError::CrcMismatch { .. }));
}

#[test]
fn compressed_frame_decode_restores_payload() {
    let raw_payload = vec![0xAB; 2048];
    let mut frame = Frame::new(
        MsgType::Data,
        0,
        12,
        1234,
        b"dev-compress".to_vec(),
        raw_payload.clone(),
    )
    .expect("build frame");

    frame
        .maybe_compress_payload(32, 1)
        .expect("compress payload");
    assert!(frame.header.flags & FLAG_IS_COMPRESSED != 0);

    let encoded = frame.encode().expect("encode compressed");
    let decoded = Frame::decode(&encoded).expect("decode compressed");
    assert_eq!(decoded.payload, raw_payload);
    assert_eq!(decoded.header.flags & FLAG_IS_COMPRESSED, 0);
}

#[test]
fn has_unknown_flags_detects_extra_bits() {
    let frame = Frame::new(
        MsgType::Heartbeat,
        FLAG_ACK_REQUIRED | (1 << 10),
        1,
        1,
        b"dev".to_vec(),
        Vec::new(),
    )
    .expect("build frame");
    assert!(frame.has_unknown_flags());
}

#[test]
fn build_dict_frame_encodes_entries() {
    let entries = vec![
        DictEntrySpec::new(1001, ValueType::F32, b"Voltage".to_vec(), b"V".to_vec()),
        DictEntrySpec::new(1002, ValueType::Bool, b"Breaker".to_vec(), b"".to_vec()),
    ];

    let frame = build_dict_frame(0, 7, 123, b"dev-01".to_vec(), 9, &entries).expect("build dict");
    assert_eq!(frame.header.msg_type, MsgType::Dict);
    assert_eq!(frame.header.seq, 7);
    assert_eq!(frame.device_id, b"dev-01".to_vec());

    let payload = frame.payload.as_ref();
    assert_eq!(
        u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]),
        9
    );
    assert_eq!(u16::from_be_bytes([payload[4], payload[5]]), 2);
}

#[test]
fn build_data_frame_encodes_point_count_and_payload() {
    let points = vec![
        PointSpec::new(
            2001,
            PointDomain::Yc,
            ValueType::F32,
            12.5_f32.to_be_bytes().to_vec(),
        ),
        PointSpec::new(2002, PointDomain::Yx, ValueType::Bool, vec![1]),
    ];
    let frame = build_data_frame(FLAG_ACK_REQUIRED, 8, 456, b"dev-02".to_vec(), &points)
        .expect("build data");

    assert_eq!(frame.header.msg_type, MsgType::Data);
    assert_eq!(frame.header.flags, FLAG_ACK_REQUIRED);
    assert_eq!(frame.header.seq, 8);

    let payload = frame.payload.as_ref();
    assert_eq!(u16::from_be_bytes([payload[0], payload[1]]), 2);
    assert_eq!(
        u32::from_be_bytes([payload[2], payload[3], payload[4], payload[5]]),
        2001
    );
    assert_eq!(payload[6], PointDomain::Yc as u8);
    assert_eq!(payload[7], ValueType::F32 as u8);
    assert_eq!(u16::from_be_bytes([payload[8], payload[9]]), 4);
}

#[test]
fn build_control_frame_encodes_timeout() {
    let spec = ControlSpec::new(11, 22, ValueType::U16, 3u16.to_be_bytes().to_vec(), 3000);
    let frame = build_control_frame(0, 9, 789, b"dev-03".to_vec(), &spec).expect("build control");

    assert_eq!(frame.header.msg_type, MsgType::Control);
    assert_eq!(frame.header.seq, 9);

    let payload = frame.payload.as_ref();
    assert_eq!(
        u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]),
        11
    );
    assert_eq!(
        u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]),
        22
    );
    assert_eq!(payload[8], ValueType::U16 as u8);
    assert_eq!(u16::from_be_bytes([payload[9], payload[10]]), 2);
    assert_eq!(
        u32::from_be_bytes([payload[13], payload[14], payload[15], payload[16]]),
        3000
    );
}
