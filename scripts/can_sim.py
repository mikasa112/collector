#!/usr/bin/env python3
"""Read CAN workbook config and generate SocketCAN simulation traffic."""

from __future__ import annotations

import argparse
import json
import random
import socket
import struct
import sys
import time
import zipfile
from dataclasses import dataclass, field
from pathlib import Path
from typing import Dict, Iterable, List, Optional
from xml.etree import ElementTree as ET


NS = {"a": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
CAN_EFF_FLAG = 0x80000000


@dataclass
class FrameDef:
    frame_id: int
    id_type: str
    dlc: int
    cycle_ms: int
    timeout_ms: int
    enable: bool


@dataclass
class SignalDef:
    frame_id: int
    start_bit: int
    bit_len: int
    byte_order: str
    data_type: str
    invalid_val: Optional[int]


@dataclass
class ExtSignalDef:
    frame_id: int
    frame_num: int
    frame_id_step: int
    each_frame_element: int
    total_element: int
    element_start_bit: int
    single_ele_bit_len: int
    byte_order: str
    data_type: str
    invalid_val: Optional[int]


@dataclass
class RuntimeFrame:
    frame_id: int
    extended: bool
    dlc: int
    period_s: float
    data: bytearray = field(default_factory=lambda: bytearray(8))
    due_at: float = 0.0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Read collector CAN config and generate simulated traffic.",
    )
    parser.add_argument(
        "--config",
        default="config/config.json",
        help="Path to collector config.json",
    )
    parser.add_argument(
        "--device",
        default="bcu1",
        help="Device id in config.json",
    )
    parser.add_argument(
        "--interface",
        help="SocketCAN interface override, default from config",
    )
    parser.add_argument(
        "--duration",
        type=float,
        default=5.0,
        help="Run duration in seconds",
    )
    parser.add_argument(
        "--step",
        type=int,
        default=1,
        help="Increment applied to generated raw values",
    )
    parser.add_argument(
        "--jitter",
        type=int,
        default=0,
        help="Random delta added to generated raw values",
    )
    parser.add_argument(
        "--print-only",
        action="store_true",
        help="Print generated frames instead of sending them",
    )
    parser.add_argument(
        "--limit",
        type=int,
        default=20,
        help="Frame preview count in print-only mode",
    )
    return parser.parse_args()


def load_project_config(path: Path) -> dict:
    with path.open("r", encoding="utf-8-sig") as fh:
        return json.load(fh)


def resolve_device_config(root: Path, config_path: Path, device_id: str) -> tuple[dict, Path]:
    project_cfg = load_project_config(config_path)
    try:
        device = project_cfg["devices"][device_id]
    except KeyError as exc:
        raise SystemExit(f"device not found in config: {device_id}") from exc
    device_config = device["config"]
    register_file = device_config.get("register_file") or device_config.get("registerFile")
    if not register_file:
        raise SystemExit(f"device {device_id} does not define registerFile")
    workbook = (config_path.parent / register_file).resolve()
    if not workbook.exists():
        workbook = (root / register_file).resolve()
    if not workbook.exists():
        raise SystemExit(f"registerFile not found: {register_file}")
    return device_config, workbook


def read_xlsx_sheet_rows(path: Path) -> Dict[str, List[List[str]]]:
    with zipfile.ZipFile(path) as zf:
        shared_strings: List[str] = []
        if "xl/sharedStrings.xml" in zf.namelist():
            root = ET.fromstring(zf.read("xl/sharedStrings.xml"))
            for item in root.findall("a:si", NS):
                shared_strings.append("".join(node.text or "" for node in item.iterfind(".//a:t", NS)))

        workbook = ET.fromstring(zf.read("xl/workbook.xml"))
        rels = ET.fromstring(zf.read("xl/_rels/workbook.xml.rels"))
        rel_map = {rel.attrib["Id"]: rel.attrib["Target"] for rel in rels}

        sheets: Dict[str, List[List[str]]] = {}
        sheets_node = workbook.find("a:sheets", NS)
        for sheet in sheets_node if sheets_node is not None else []:
            name = sheet.attrib["name"]
            rel_id = sheet.attrib["{http://schemas.openxmlformats.org/officeDocument/2006/relationships}id"]
            target = rel_map[rel_id]
            rows_xml = ET.fromstring(zf.read(f"xl/{target}"))
            rows: List[List[str]] = []
            for row in rows_xml.iterfind(".//a:sheetData/a:row", NS):
                values: List[str] = []
                for cell in row.findall("a:c", NS):
                    ref = cell.attrib.get("r", "")
                    col_idx = excel_col_to_index(ref)
                    while len(values) < col_idx:
                        values.append("")
                    text = ""
                    value_node = cell.find("a:v", NS)
                    if value_node is not None:
                        text = value_node.text or ""
                        if cell.attrib.get("t") == "s":
                            text = shared_strings[int(text)]
                    values.append(text)
                rows.append(values)
            sheets[name] = rows
        return sheets


def excel_col_to_index(ref: str) -> int:
    col = "".join(ch for ch in ref if ch.isalpha())
    idx = 0
    for ch in col:
        idx = idx * 26 + (ord(ch.upper()) - ord("A") + 1)
    return max(idx - 1, 0)


def parse_hex(value: str) -> int:
    value = (value or "").strip()
    if not value:
        return 0
    return int(value, 16) if value.lower().startswith("0x") else int(float(value))


def parse_optional_hex(value: str) -> Optional[int]:
    value = (value or "").strip()
    if not value:
        return None
    return parse_hex(value)


def parse_frames(rows: List[List[str]]) -> Dict[int, FrameDef]:
    frames: Dict[int, FrameDef] = {}
    for row in rows[1:]:
        if len(row) < 11 or not row[2]:
            continue
        frame_id = parse_hex(row[2])
        frames[frame_id] = FrameDef(
            frame_id=frame_id,
            id_type=row[3].strip().lower(),
            dlc=int(float(row[4] or 8)),
            cycle_ms=int(float(row[5] or 0)),
            timeout_ms=int(float(row[6] or 0)),
            enable=int(float(row[10] or 0)) != 0,
        )
    return frames


def parse_signals(rows: List[List[str]]) -> Dict[int, List[SignalDef]]:
    signals: Dict[int, List[SignalDef]] = {}
    for row in rows[1:]:
        if len(row) < 12 or not row[2]:
            continue
        frame_id = parse_hex(row[2])
        signals.setdefault(frame_id, []).append(
            SignalDef(
                frame_id=frame_id,
                start_bit=int(float(row[4] or 0)),
                bit_len=int(float(row[5] or 0)),
                byte_order=row[6].strip().lower(),
                data_type=row[7].strip().lower(),
                invalid_val=parse_optional_hex(row[11] if len(row) > 11 else ""),
            )
        )
    return signals


def parse_ext_signals(rows: List[List[str]]) -> Dict[int, List[ExtSignalDef]]:
    signals: Dict[int, List[ExtSignalDef]] = {}
    for row in rows[1:]:
        if len(row) < 16 or not row[3]:
            continue
        frame_id = parse_hex(row[3])
        signals.setdefault(frame_id, []).append(
            ExtSignalDef(
                frame_id=frame_id,
                frame_num=int(float(row[4] or 0)),
                frame_id_step=max(int(float(row[5] or 1)), 1),
                each_frame_element=int(float(row[6] or 0)),
                total_element=int(float(row[7] or 0)),
                element_start_bit=int(float(row[8] or 0)),
                single_ele_bit_len=int(float(row[9] or 0)),
                byte_order=row[10].strip().lower(),
                data_type=row[11].strip().lower(),
                invalid_val=parse_optional_hex(row[15] if len(row) > 15 else ""),
            )
        )
    return signals


def build_runtime_frames(
    frame_defs: Dict[int, FrameDef],
    signals: Dict[int, List[SignalDef]],
    ext_signals: Dict[int, List[ExtSignalDef]],
    step: int,
    jitter: int,
) -> List[RuntimeFrame]:
    runtime_by_id: Dict[int, RuntimeFrame] = {}
    raw_seed = 1

    for frame_id, frame_def in frame_defs.items():
        if not frame_def.enable:
            continue
        runtime_by_id.setdefault(
            frame_id,
            RuntimeFrame(
                frame_id=frame_id,
                extended=(frame_def.id_type == "extended"),
                dlc=frame_def.dlc,
                period_s=compute_period(frame_def),
            ),
        )

        for sig in signals.get(frame_id, []):
            raw_seed = fill_normal_signal(runtime_by_id[frame_id], sig, raw_seed, step, jitter)

        for ext_sig in ext_signals.get(frame_id, []):
            raw_seed = fill_ext_signals(runtime_by_id, frame_def, ext_sig, raw_seed, step, jitter)

    now = time.monotonic()
    for frame in runtime_by_id.values():
        frame.due_at = now
    return sorted(runtime_by_id.values(), key=lambda item: item.frame_id)


def compute_period(frame_def: FrameDef) -> float:
    period_ms = frame_def.cycle_ms or 1000
    if frame_def.timeout_ms > 0:
        period_ms = min(period_ms, max(frame_def.timeout_ms // 2, 1))
    return max(period_ms, 1) / 1000.0


def fill_normal_signal(
    runtime: RuntimeFrame,
    signal: SignalDef,
    raw_seed: int,
    step: int,
    jitter: int,
) -> int:
    raw_value = pick_raw_value(signal.bit_len, signal.invalid_val, raw_seed, step, jitter)
    insert_raw(runtime.data, signal.start_bit, signal.bit_len, signal.byte_order, raw_value)
    return raw_seed + max(step, 1)


def fill_ext_signals(
    runtime_by_id: Dict[int, RuntimeFrame],
    frame_def: FrameDef,
    signal: ExtSignalDef,
    raw_seed: int,
    step: int,
    jitter: int,
) -> int:
    for frame_idx in range(signal.frame_num):
        raw_id = signal.frame_id + frame_idx * signal.frame_id_step
        runtime = runtime_by_id.setdefault(
            raw_id,
            RuntimeFrame(
                frame_id=raw_id,
                extended=(frame_def.id_type == "extended"),
                dlc=frame_def.dlc,
                period_s=compute_period(frame_def),
            ),
        )
        start_element = frame_idx * signal.each_frame_element
        for element_idx in range(signal.each_frame_element):
            if start_element + element_idx >= signal.total_element:
                break
            raw_value = pick_raw_value(
                signal.single_ele_bit_len,
                signal.invalid_val,
                raw_seed,
                step,
                jitter,
            )
            bit_offset = signal.element_start_bit + element_idx * signal.single_ele_bit_len
            insert_raw(
                runtime.data,
                bit_offset,
                signal.single_ele_bit_len,
                signal.byte_order,
                raw_value,
            )
            raw_seed += max(step, 1)
    return raw_seed


def pick_raw_value(bit_len: int, invalid_val: Optional[int], seed: int, step: int, jitter: int) -> int:
    max_val = (1 << bit_len) - 1
    value = seed % (max_val + 1)
    if jitter > 0:
        value = (value + random.randint(0, jitter)) % (max_val + 1)
    if invalid_val is not None and value == invalid_val:
        value = (value + max(step, 1)) % (max_val + 1)
    return value


def insert_raw(data: bytearray, start_bit: int, bit_len: int, byte_order: str, raw_value: int) -> None:
    if byte_order == "intel":
        insert_intel(data, start_bit, bit_len, raw_value)
        return
    insert_motorola(data, start_bit, bit_len, raw_value)


def insert_intel(data: bytearray, start_bit: int, bit_len: int, raw_value: int) -> None:
    for bit_idx in range(bit_len):
        pos = start_bit + bit_idx
        byte_idx = pos // 8
        bit_in_byte = pos % 8
        bit = (raw_value >> bit_idx) & 1
        set_bit(data, byte_idx, bit_in_byte, bit)


def insert_motorola(data: bytearray, start_bit: int, bit_len: int, raw_value: int) -> None:
    pos = start_bit
    for bit_idx in range(bit_len):
        shift = bit_len - bit_idx - 1
        bit = (raw_value >> shift) & 1
        byte_idx = pos // 8
        bit_in_byte = pos % 8
        set_bit(data, byte_idx, bit_in_byte, bit)
        pos = pos + 15 if pos % 8 == 0 else pos - 1


def set_bit(data: bytearray, byte_idx: int, bit_in_byte: int, bit: int) -> None:
    mask = 1 << bit_in_byte
    if bit:
        data[byte_idx] |= mask
    else:
        data[byte_idx] &= ~mask


def send_frames(interface: str, frames: List[RuntimeFrame], duration_s: float) -> None:
    sock = socket.socket(socket.AF_CAN, socket.SOCK_RAW, socket.CAN_RAW)
    sock.bind((interface,))
    end_at = time.monotonic() + duration_s

    try:
        while time.monotonic() < end_at:
            frame = min(frames, key=lambda item: item.due_at)
            now = time.monotonic()
            if frame.due_at > now:
                time.sleep(min(frame.due_at - now, 0.001))
                continue
            write_can_frame(sock, frame)
            frame.due_at = now + frame.period_s
    finally:
        sock.close()


def write_can_frame(sock: socket.socket, frame: RuntimeFrame) -> None:
    can_id = frame.frame_id | (CAN_EFF_FLAG if frame.extended else 0)
    payload = bytes(frame.data[: frame.dlc]).ljust(8, b"\x00")
    packet = struct.pack("=IB3x8s", can_id, frame.dlc, payload)
    sock.send(packet)


def print_frames(frames: Iterable[RuntimeFrame], limit: int) -> None:
    for index, frame in enumerate(frames):
        if index >= limit:
            break
        can_id = f"{frame.frame_id:08X}" if frame.extended else f"{frame.frame_id:03X}"
        print(
            f"{index + 1:03d} id=0x{can_id} dlc={frame.dlc} period={frame.period_s * 1000:.0f}ms data={frame.data[:frame.dlc].hex().upper()}"
        )


def main() -> int:
    args = parse_args()
    root = Path(__file__).resolve().parents[1]
    config_path = (root / args.config).resolve() if not Path(args.config).is_absolute() else Path(args.config)
    dev_cfg, workbook = resolve_device_config(root, config_path, args.device)
    sheets = read_xlsx_sheet_rows(workbook)
    frame_defs = parse_frames(sheets["报文"])
    signals = parse_signals(sheets["信号"])
    ext_signals = parse_ext_signals(sheets["信号_扩展"])
    frames = build_runtime_frames(frame_defs, signals, ext_signals, args.step, args.jitter)

    if not frames:
        raise SystemExit("no enabled CAN frames found")

    interface = args.interface or dev_cfg["interface"]
    print(
        f"device={args.device} interface={interface} workbook={workbook} frames={len(frames)} duration={args.duration}s",
        file=sys.stderr,
    )

    if args.print_only:
        print_frames(frames, args.limit)
        return 0

    send_frames(interface, frames, args.duration)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
