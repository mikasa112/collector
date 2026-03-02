#!/usr/bin/env python3
import argparse
import socket
import time
import zlib

try:
    import zstandard as zstd
except ImportError:
    zstd = None

MAGIC = 0xCC01
VERSION = 0x01
HEADER_FIXED_LEN = 26
FLAG_ACK_REQUIRED = 1 << 0
FLAG_IS_COMPRESSED = 1 << 3
MSG_TYPE_DICT = 5
MSG_TYPE_DATA = 1
MSG_TYPE_CONTROL = 2
MSG_TYPE_HEARTBEAT = 6

MSG_TYPE_NAME = {
    MSG_TYPE_DATA: "Data",
    MSG_TYPE_CONTROL: "Control",
    MSG_TYPE_DICT: "Dict",
    MSG_TYPE_HEARTBEAT: "Heartbeat",
    7: "Ack",
    255: "Error",
}

DOMAIN_NAME = {
    0: "UNKNOWN",
    1: "YK",
    2: "YX",
    3: "YT",
    4: "YC",
}

VALUE_TYPE_NAME = {
    1: "U8",
    2: "I8",
    3: "I16",
    4: "I32",
    5: "U16",
    6: "U32",
    7: "F32",
    8: "BOOL",
    9: "UTF8_STRING",
}


def decode_typed_value(value_type: int, raw: bytes):
    if value_type == 1 and len(raw) == 1:
        return int(raw[0])
    if value_type == 2 and len(raw) == 1:
        return int.from_bytes(raw, "big", signed=True)
    if value_type == 3 and len(raw) == 2:
        return int.from_bytes(raw, "big", signed=True)
    if value_type == 4 and len(raw) == 4:
        return int.from_bytes(raw, "big", signed=True)
    if value_type == 5 and len(raw) == 2:
        return int.from_bytes(raw, "big", signed=False)
    if value_type == 6 and len(raw) == 4:
        return int.from_bytes(raw, "big", signed=False)
    if value_type == 7 and len(raw) == 4:
        import struct

        return struct.unpack(">f", raw)[0]
    if value_type == 8 and len(raw) == 1:
        return raw[0] != 0
    if value_type == 9:
        return raw.decode("utf-8", errors="replace")
    return f"0x{raw.hex()}"


def encode_typed_value(value_type: int, value_text: str):
    if value_type == 1:
        return int(value_text).to_bytes(1, "big", signed=False)
    if value_type == 2:
        return int(value_text).to_bytes(1, "big", signed=True)
    if value_type == 3:
        return int(value_text).to_bytes(2, "big", signed=True)
    if value_type == 4:
        return int(value_text).to_bytes(4, "big", signed=True)
    if value_type == 5:
        return int(value_text).to_bytes(2, "big", signed=False)
    if value_type == 6:
        return int(value_text).to_bytes(4, "big", signed=False)
    if value_type == 7:
        import struct

        return struct.pack(">f", float(value_text))
    if value_type == 8:
        v = value_text.strip().lower()
        return bytes([1 if v in ("1", "true", "yes", "on") else 0])
    if value_type == 9:
        return value_text.encode("utf-8")
    raise ValueError(f"unsupported value_type: {value_type}")


def alloc_seq(seq_ref):
    out = seq_ref[0]
    seq_ref[0] = (seq_ref[0] + 1) & 0xFFFFFFFF
    if seq_ref[0] == 0:
        seq_ref[0] = 1
    return out


def build_frame(
    msg_type: int,
    flags: int,
    seq: int,
    timestamp_ms: int,
    device_id: str,
    body: bytes,
):
    dev = device_id.encode("utf-8")
    payload = dev + body
    header = b"".join(
        [
            MAGIC.to_bytes(2, "big"),
            VERSION.to_bytes(1, "big"),
            msg_type.to_bytes(1, "big"),
            flags.to_bytes(2, "big"),
            HEADER_FIXED_LEN.to_bytes(2, "big"),
            len(payload).to_bytes(4, "big"),
            seq.to_bytes(4, "big"),
            timestamp_ms.to_bytes(8, "big"),
            len(dev).to_bytes(2, "big"),
        ]
    )
    frame_wo_crc = header + payload
    crc = (zlib.crc32(frame_wo_crc) & 0xFFFFFFFF).to_bytes(4, "big")
    return frame_wo_crc + crc


def build_heartbeat_frame(seq: int, device_id: str, status: int, uptime_s: int):
    body = bytes([status & 0xFF]) + int(uptime_s).to_bytes(4, "big", signed=False)
    return build_frame(
        msg_type=MSG_TYPE_HEARTBEAT,
        flags=0,
        seq=seq,
        timestamp_ms=int(time.time() * 1000),
        device_id=device_id,
        body=body,
    )


def build_ack_frame(
    seq: int, device_id: str, ack_seq: int, code: int = 0, msg: bytes = b""
):
    if len(msg) > 255:
        msg = msg[:255]
    body = (
        int(ack_seq).to_bytes(4, "big", signed=False)
        + int(code).to_bytes(2, "big", signed=False)
        + len(msg).to_bytes(1, "big", signed=False)
        + msg
    )
    return build_frame(
        msg_type=7,
        flags=0,
        seq=seq,
        timestamp_ms=int(time.time() * 1000),
        device_id=device_id,
        body=body,
    )


def build_control_frame(
    seq: int,
    device_id: str,
    cmd_id: int,
    point_id: int,
    value_type: int,
    value_raw: bytes,
    timeout_ms: int,
    ack_required: bool = True,
):
    body = (
        int(cmd_id).to_bytes(4, "big", signed=False)
        + int(point_id).to_bytes(4, "big", signed=False)
        + int(value_type).to_bytes(1, "big", signed=False)
        + len(value_raw).to_bytes(2, "big", signed=False)
        + value_raw
        + int(timeout_ms).to_bytes(4, "big", signed=False)
    )
    flags = FLAG_ACK_REQUIRED if ack_required else 0
    return build_frame(
        msg_type=MSG_TYPE_CONTROL,
        flags=flags,
        seq=seq,
        timestamp_ms=int(time.time() * 1000),
        device_id=device_id,
        body=body,
    )


def decode_one_frame(buf: bytes):
    if len(buf) < HEADER_FIXED_LEN:
        return None

    magic = int.from_bytes(buf[0:2], "big")
    if magic != MAGIC:
        raise ValueError(f"bad magic: {magic:#06x}")

    version = buf[2]
    if version != VERSION:
        raise ValueError(f"unsupported version: {version}")

    msg_type = buf[3]
    flags = int.from_bytes(buf[4:6], "big")
    header_len = int.from_bytes(buf[6:8], "big")
    payload_len = int.from_bytes(buf[8:12], "big")
    seq = int.from_bytes(buf[12:16], "big")
    timestamp_ms = int.from_bytes(buf[16:24], "big")
    device_id_len = int.from_bytes(buf[24:26], "big")

    total_len = header_len + payload_len + 4
    if len(buf) < total_len:
        return None

    body = buf[: total_len - 4]
    crc_expected = int.from_bytes(buf[total_len - 4 : total_len], "big")
    crc_actual = zlib.crc32(body) & 0xFFFFFFFF
    if crc_actual != crc_expected:
        raise ValueError(
            f"crc mismatch: expected={crc_expected:#010x}, actual={crc_actual:#010x}"
        )

    payload_start = header_len
    payload_end = payload_start + payload_len
    device_id_end = payload_start + device_id_len
    if device_id_end > payload_end:
        raise ValueError("device_id_len exceeds payload")

    device_id = buf[payload_start:device_id_end].decode("utf-8", errors="replace")
    payload = buf[device_id_end:payload_end]
    is_compressed = (flags & FLAG_IS_COMPRESSED) != 0
    if is_compressed:
        if zstd is None:
            raise ValueError(
                "frame is compressed(zstd) but python package 'zstandard' is not installed"
            )
        try:
            payload = zstd.ZstdDecompressor().decompress(payload)
        except zstd.ZstdError as e:
            raise ValueError(f"zstd decompress failed: {e}") from e

    frame = {
        "msg_type": msg_type,
        "flags": flags,
        "is_compressed": is_compressed,
        "seq": seq,
        "timestamp_ms": timestamp_ms,
        "device_id": device_id,
        "payload": payload,
        "total_len": total_len,
    }
    return frame


def parse_dict_payload(payload: bytes):
    if len(payload) < 6:
        raise ValueError("dict payload too short")

    off = 0
    dict_version = int.from_bytes(payload[off : off + 4], "big")
    off += 4
    entry_count = int.from_bytes(payload[off : off + 2], "big")
    off += 2

    entries = []
    for _ in range(entry_count):
        if off + 8 > len(payload):
            raise ValueError("dict entry truncated")

        point_id = int.from_bytes(payload[off : off + 4], "big")
        off += 4
        name_len = int.from_bytes(payload[off : off + 2], "big")
        off += 2

        if off + name_len + 2 > len(payload):
            raise ValueError("dict name truncated")

        name = payload[off : off + name_len].decode("utf-8", errors="replace")
        off += name_len

        unit_len = payload[off]
        off += 1

        if off + unit_len + 1 > len(payload):
            raise ValueError("dict unit/value_type truncated")

        unit = payload[off : off + unit_len].decode("utf-8", errors="replace")
        off += unit_len

        value_type = payload[off]
        off += 1

        entries.append(
            {
                "point_id": point_id,
                "name": name,
                "unit": unit,
                "value_type": value_type,
                "value_type_name": VALUE_TYPE_NAME.get(
                    value_type, f"UNKNOWN({value_type})"
                ),
            }
        )

    return {
        "dict_version": dict_version,
        "entry_count": entry_count,
        "entries": entries,
    }


def parse_points_payload(payload: bytes):
    if len(payload) < 2:
        raise ValueError("points payload too short")

    off = 0
    point_count = int.from_bytes(payload[off : off + 2], "big")
    off += 2

    points = []
    for _ in range(point_count):
        if off + 8 > len(payload):
            raise ValueError("point entry truncated")

        point_id = int.from_bytes(payload[off : off + 4], "big")
        off += 4
        domain = payload[off]
        off += 1
        value_type = payload[off]
        off += 1
        value_len = int.from_bytes(payload[off : off + 2], "big")
        off += 2

        if off + value_len > len(payload):
            raise ValueError("point value truncated")

        value_raw = payload[off : off + value_len]
        off += value_len
        points.append(
            {
                "point_id": point_id,
                "domain": domain,
                "domain_name": DOMAIN_NAME.get(domain, f"UNKNOWN({domain})"),
                "value_type": value_type,
                "value_type_name": VALUE_TYPE_NAME.get(
                    value_type, f"UNKNOWN({value_type})"
                ),
                "value": decode_typed_value(value_type, value_raw),
            }
        )

    return {"point_count": point_count, "points": points}


def parse_control_payload(payload: bytes):
    if len(payload) < 15:
        raise ValueError("control payload too short")

    off = 0
    cmd_id = int.from_bytes(payload[off : off + 4], "big")
    off += 4
    point_id = int.from_bytes(payload[off : off + 4], "big")
    off += 4
    value_type = payload[off]
    off += 1
    value_len = int.from_bytes(payload[off : off + 2], "big")
    off += 2
    if off + value_len + 4 > len(payload):
        raise ValueError("control value/timeout truncated")
    value_raw = payload[off : off + value_len]
    off += value_len
    timeout_ms = int.from_bytes(payload[off : off + 4], "big")

    return {
        "cmd_id": cmd_id,
        "point_id": point_id,
        "value_type": value_type,
        "value_type_name": VALUE_TYPE_NAME.get(value_type, f"UNKNOWN({value_type})"),
        "value": decode_typed_value(value_type, value_raw),
        "timeout_ms": timeout_ms,
    }


def main():
    parser = argparse.ArgumentParser(
        description="Connect to collector TCP dock server and print received Dict frames"
    )
    parser.add_argument("--host", default="127.0.0.1", help="server host")
    parser.add_argument("--port", type=int, default=8083, help="server port")
    parser.add_argument(
        "--timeout",
        type=float,
        default=0,
        help="seconds of no incoming frame before stopping (0 means no limit)",
    )
    parser.add_argument(
        "--max-dicts",
        type=int,
        default=0,
        help="stop after receiving this many Dict frames (0 means no limit)",
    )
    parser.add_argument(
        "--max-pushes",
        type=int,
        default=0,
        help="stop after receiving this many push frames (四遥), 0 means no limit",
    )
    parser.add_argument(
        "--device-id", default="test-client", help="device id used for heartbeat frames"
    )
    parser.add_argument(
        "--heartbeat-interval",
        type=float,
        default=10.0,
        help="seconds between heartbeats sent by this client (<=0 disables)",
    )
    parser.add_argument(
        "--heartbeat-status",
        type=int,
        default=2,
        help="heartbeat status (0:init,1:ready,2:running,3:degraded)",
    )
    parser.add_argument(
        "--control-point-id",
        type=lambda s: int(s, 0),
        help="send one control test after connect, point id (supports hex, e.g. 0x00010008)",
    )
    parser.add_argument(
        "--control-value-type",
        type=int,
        default=5,
        help="control test value type (1..9)",
    )
    parser.add_argument(
        "--control-value",
        help="control test value text, e.g. 1 / -3 / 12.5 / true",
    )
    parser.add_argument(
        "--control-timeout-ms",
        type=int,
        default=3000,
        help="control test timeout_ms",
    )
    parser.add_argument(
        "--control-cmd-id", type=int, default=1, help="control test cmd id"
    )
    args = parser.parse_args()
    if (args.control_point_id is None) != (args.control_value is None):
        parser.error("--control-point-id and --control-value must be provided together")
    if args.control_value_type < 1 or args.control_value_type > 9:
        parser.error("--control-value-type must be in 1..9")

    print(f"Connecting to {args.host}:{args.port} ...")
    sock = socket.create_connection((args.host, args.port), timeout=3)
    sock.settimeout(0.5)

    pending = b""
    dict_count = 0
    push_count = 0
    hb_sent_count = 0
    control_sent_count = 0
    last_data_at = time.time()
    start_at = time.time()
    next_hb_at = start_at + args.heartbeat_interval
    out_seq = [1]
    sent_control_test = False

    try:
        while True:
            if args.max_dicts > 0 and dict_count >= args.max_dicts:
                print(f"Reached max_dicts={args.max_dicts}, stopping")
                break
            if args.max_pushes > 0 and push_count >= args.max_pushes:
                print(f"Reached max_pushes={args.max_pushes}, stopping")
                break
            if args.timeout > 0 and time.time() - last_data_at >= args.timeout:
                print(f"No incoming frame for {args.timeout}s, stopping")
                break

            if (
                not sent_control_test
                and args.control_point_id is not None
                and args.control_value is not None
            ):
                seq = alloc_seq(out_seq)
                value_raw = encode_typed_value(
                    args.control_value_type, args.control_value
                )
                ctrl = build_control_frame(
                    seq=seq,
                    device_id=args.device_id,
                    cmd_id=args.control_cmd_id,
                    point_id=args.control_point_id,
                    value_type=args.control_value_type,
                    value_raw=value_raw,
                    timeout_ms=args.control_timeout_ms,
                    ack_required=True,
                )
                sock.sendall(ctrl)
                control_sent_count += 1
                sent_control_test = True
                print(
                    f"send control#{control_sent_count} seq={seq} device={args.device_id} "
                    f"cmd_id={args.control_cmd_id} point_id={args.control_point_id} "
                    f"value_type={args.control_value_type} value={args.control_value}"
                )

            now = time.time()
            if args.heartbeat_interval > 0 and now >= next_hb_at:
                seq = alloc_seq(out_seq)
                uptime_s = int(now - start_at)
                hb = build_heartbeat_frame(
                    seq=seq,
                    device_id=args.device_id,
                    status=args.heartbeat_status,
                    uptime_s=uptime_s,
                )
                sock.sendall(hb)
                hb_sent_count += 1
                print(
                    f"send heartbeat#{hb_sent_count} seq={seq} "
                    f"device={args.device_id} status={args.heartbeat_status} "
                    f"uptime_s={uptime_s}"
                )
                next_hb_at = now + args.heartbeat_interval

            try:
                chunk = sock.recv(4096)
            except socket.timeout:
                continue

            if not chunk:
                print("Connection closed by server")
                break

            last_data_at = time.time()
            pending += chunk

            while True:
                if len(pending) < HEADER_FIXED_LEN:
                    break

                try:
                    frame = decode_one_frame(pending)
                except ValueError as e:
                    # Resync by dropping one byte on framing error.
                    print(f"Frame parse error: {e}; resyncing")
                    pending = pending[1:]
                    continue

                if frame is None:
                    break

                pending = pending[frame["total_len"] :]

                msg_type = frame["msg_type"]
                msg_type_name = MSG_TYPE_NAME.get(msg_type, f"UNKNOWN({msg_type})")
                if frame["flags"] & FLAG_ACK_REQUIRED:
                    ack_send_seq = alloc_seq(out_seq)
                    ack = build_ack_frame(
                        seq=ack_send_seq,
                        device_id=frame["device_id"],
                        ack_seq=frame["seq"],
                    )
                    sock.sendall(ack)
                    print(
                        f"send ack seq={ack_send_seq} ack_seq={frame['seq']} "
                        f"for type={msg_type_name} device={frame['device_id']}"
                    )

                if msg_type == MSG_TYPE_DICT:
                    dict_count += 1
                    parsed = parse_dict_payload(frame["payload"])
                    print("=" * 72)
                    print(
                        f"Dict#{dict_count} seq={frame['seq']} device={frame['device_id']} "
                        f"version={parsed['dict_version']} entries={parsed['entry_count']}"
                    )
                    for i, entry in enumerate(parsed["entries"], 1):
                        print(
                            f"  {i}. point_id={entry['point_id']} "
                            f"name={entry['name']} unit={entry['unit']} "
                            f"value_type={entry['value_type_name']}"
                        )
                    continue

                if msg_type == MSG_TYPE_DATA:
                    push_count += 1
                    parsed = parse_points_payload(frame["payload"])
                    print("=" * 72)
                    print(
                        f"{msg_type_name}#{push_count} seq={frame['seq']} "
                        f"device={frame['device_id']} points={parsed['point_count']}"
                    )
                    for i, point in enumerate(parsed["points"], 1):
                        print(
                            f"  {i}. point_id={point['point_id']} "
                            f"domain={point['domain_name']} "
                            f"value_type={point['value_type_name']} "
                            f"value={point['value']}"
                        )
                    continue

                if msg_type == MSG_TYPE_CONTROL:
                    push_count += 1
                    parsed = parse_control_payload(frame["payload"])
                    print("=" * 72)
                    print(
                        f"{msg_type_name}#{push_count} seq={frame['seq']} "
                        f"device={frame['device_id']} cmd_id={parsed['cmd_id']} "
                        f"point_id={parsed['point_id']} "
                        f"value_type={parsed['value_type_name']} "
                        f"value={parsed['value']} timeout_ms={parsed['timeout_ms']}"
                    )
                    continue

                print(
                    f"recv frame type={msg_type_name} seq={frame['seq']} "
                    f"device={frame['device_id']} payload_len={len(frame['payload'])} "
                    f"compressed={frame['is_compressed']}"
                )

    except KeyboardInterrupt:
        print("\nInterrupted by user (Ctrl+C), exiting")
    finally:
        print(
            f"Done, received {dict_count} Dict frame(s), {push_count} push frame(s), "
            f"sent {hb_sent_count} heartbeat frame(s), "
            f"sent {control_sent_count} control frame(s)"
        )
        sock.close()


if __name__ == "__main__":
    main()
