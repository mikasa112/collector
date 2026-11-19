#!/usr/bin/env python3
"""开发机 Modbus 设备模拟器：为 config.json 的 devices 下 ModbusTCP 设备提供模拟数据。

读取 config.json 中每个 com_type=ModbusTCP 的设备，按其 register_file 点表
（遥测/遥信/遥调/遥控）生成寄存器镜像，并在设备配置的 port 上起 Modbus TCP server，
collector 无需改动即可采到数据。纯标准库。

- 遥测：按单位给出合理基准值（V/A/kW/Hz/℃/%/SOC...），并做缓慢波动；
        值按 collector 的解码公式 `值 = 原始值 × 系数 + 偏移量` 反推原始寄存器值。
- 遥信：有"状态字"取第一个状态，否则为 0；告警位默认 0。
- 遥调/遥控：可被 collector 下发写入（FC05/06/15/16），写入后回读一致。
- 支持 重复次数/地址步长 展开、BA/CDAB 字节序/字序、0x 十六进制地址。

用法:
  python3 scripts/mock_modbus_devices.py                    # 模拟 config/config.json 全部 ModbusTCP 设备
  python3 scripts/mock_modbus_devices.py --devices pcs bms  # 只模拟指定设备 id
  python3 scripts/mock_modbus_devices.py --check            # 只解析点表并打印概要，不监听
  python3 scripts/mock_modbus_devices.py -v                 # 打印收到的写入（遥调/遥控下发）
  python3 scripts/mock_modbus_devices.py --set pcs:pcsPortAPhVolt=230   # 指定某点的工程值
"""

from __future__ import annotations

import argparse
import asyncio
import json
import math
import random
import re
import struct
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from export_config_from_xlsm import XlsmWorkbook  # noqa: E402  复用现有的 xlsx 解析

SHEETS = ("遥测", "遥信", "遥调", "遥控")
RANGES = {
    "bool": (0, 1), "U16": (0, 0xFFFF), "I16": (-0x8000, 0x7FFF),
    "U32": (0, 0xFFFFFFFF), "I32": (-0x80000000, 0x7FFFFFFF),
}
DTYPE_ALIAS = {"Bool": "bool", "S16": "I16", "s16": "I16", "S32": "I32", "s32": "I32"}
WIDTH = {"U32": 2, "I32": 2}
RTYPE_FC = {"Coils": 1, "DiscreteInputs": 2, "HoldingRegisters": 3, "InputRegisters": 4}

# 单位 -> 基准工程值
UNIT_BASE = {
    "v": 380.0, "kv": 10.0, "a": 10.0, "ka": 0.1, "w": 5000.0, "kw": 50.0, "mw": 0.05,
    "var": 500.0, "kvar": 5.0, "va": 5500.0, "kva": 55.0, "hz": 50.0,
    "℃": 25.0, "°c": 25.0, "c": 25.0, "%": 50.0, "%rh": 55.0, "ah": 100.0, "kwh": 1000.0,
    "mv": 3300.0, "ω": 10.0, "mω": 10.0, "kω": 500.0, "h": 100.0, "pf": 0.95,
    "bar": 2.0, "kpa": 200.0, "mpa": 0.2, "l/min": 30.0, "rpm": 1500.0, "m3/h": 10.0,
}
NAME_BASE = (("soc", 60.0), ("soh", 98.0), ("功率因数", 0.95))


@dataclass
class Point:
    sheet: str
    name: str
    key: str
    dtype: str
    addr: int
    rtype: str
    qty: int
    order: str
    scale: float
    offset: float
    unit: str
    base: float          # 基准工程值
    dynamic: bool        # 是否随时间波动
    phase: float = 0.0
    period: float = 60.0


@dataclass
class Unit:
    regs: dict[int, dict[int, int]] = field(
        default_factory=lambda: {1: {}, 2: {}, 3: {}, 4: {}})
    points: list[Point] = field(default_factory=list)
    names: dict[tuple[int, int], str] = field(default_factory=dict)


def num(text: str, default: float | None = None) -> float | None:
    text = (text or "").strip()
    if not text:
        return default
    try:
        return float(int(text, 16)) if text.lower().startswith("0x") else float(text)
    except ValueError:
        return default


def first_state(words: str) -> int | None:
    m = re.match(r"\s*(-?\d+)", words or "")
    return int(m.group(1)) if m else None


def base_value(sheet: str, unit: str, name: str, words: str) -> tuple[float, bool]:
    """返回 (基准工程值, 是否波动)。"""
    state = first_state(words)
    if state is not None:
        return float(state), False
    if sheet in ("遥信", "遥控"):
        return 0.0, False
    for token, value in NAME_BASE:
        if token in name.lower():
            return value, sheet == "遥测"
    value = UNIT_BASE.get(unit.strip().lower())
    if value is None:
        return 0.0, False
    return value, sheet == "遥测"


def load_points(register_file: Path, tag: str) -> list[Point]:
    wb = XlsmWorkbook(register_file)
    result: list[Point] = []
    for sheet in SHEETS:
        if sheet not in wb.sheet_targets:
            continue
        for idx, row in enumerate(wb.sheet_rows(sheet), start=2):
            if (num(row.get("启用", ""), 1.0) or 0) == 0:
                continue  # 启用=0 禁用；空视为启用，与 collector 一致
            dtype = DTYPE_ALIAS.get(row.get("数据类型", "").strip(), row.get("数据类型", "").strip())
            rtype = row.get("寄存器类型", "").strip()
            addr = num(row.get("寄存器地址", ""))
            qty = num(row.get("数量", ""), 1.0)
            if dtype not in RANGES or rtype not in RTYPE_FC or addr is None or not qty:
                print(f"  [{tag}] {sheet} 第{idx}行跳过: 数据类型/寄存器类型/地址无效", file=sys.stderr)
                continue
            unit = row.get("单位", "")
            name = row.get("点位名称", "")
            base, dynamic = base_value(sheet, unit, name, row.get("状态字", ""))
            repeat = int(num(row.get("重复次数", ""), 1.0) or 1)
            step = int(num(row.get("地址步长", ""), 0.0) or 0)
            for n in range(max(repeat, 1)):
                result.append(Point(
                    sheet=sheet, name=name, key=row.get("键", ""), dtype=dtype,
                    addr=int(addr) + n * step, rtype=rtype, qty=int(qty),
                    order=row.get("字节序/字序", "").strip() or "AB",
                    scale=num(row.get("系数", row.get("缩放", "")), 1.0) or 0.0,
                    offset=num(row.get("偏移量", ""), 0.0) or 0.0,
                    unit=unit, base=base, dynamic=dynamic,
                    phase=random.uniform(0, 2 * math.pi), period=random.uniform(30, 120),
                ))
    return result


def encode(value: float, p: Point) -> list[int]:
    """工程值 -> 寄存器字列表（长度 = p.qty；数组点每个元素同值）。"""
    if p.dtype == "bool":
        return [1 if value else 0] * p.qty
    lo, hi = RANGES[p.dtype]
    raw = round((value - p.offset) / (p.scale if p.scale else 1.0))
    raw = max(lo, min(hi, raw)) & (0xFFFFFFFF if p.dtype in WIDTH else 0xFFFF)
    if p.dtype in WIDTH:
        words = [(raw >> 16) & 0xFFFF, raw & 0xFFFF]
        if p.order == "CDAB":
            words.reverse()
    else:
        words = [((raw & 0xFF) << 8) | (raw >> 8) if p.order == "BA" else raw]
    return words * (p.qty // len(words))


def current_value(p: Point, now: float, overrides: dict[str, float]) -> float:
    if p.key in overrides:
        return overrides[p.key]
    if not p.dynamic or p.base == 0:
        return p.base
    return p.base * (1 + 0.02 * math.sin(2 * math.pi * now / p.period + p.phase)
                     + random.uniform(-0.003, 0.003))


def fill(unit: Unit, p: Point, now: float, overrides: dict[str, float]) -> None:
    words = encode(current_value(p, now, overrides), p)
    table = unit.regs[RTYPE_FC[p.rtype]]
    for i, w in enumerate(words):
        table[p.addr + i] = w


class ModbusDevice:
    """一个 TCP 端口，可承载多个从站号。"""

    def __init__(self, bind: str, port: int, verbose: bool) -> None:
        self.bind, self.port, self.verbose = bind, port, verbose
        self.units: dict[int, Unit] = {}
        self.labels: dict[int, str] = {}

    def log_write(self, uid: int, fc: int, addr: int, values: list[int]) -> None:
        if not self.verbose:
            return
        unit = self.units[uid]
        kind = {5: 1, 15: 1, 6: 3, 16: 3}[fc]
        for i, v in enumerate(values):
            name = unit.names.get((kind, addr + i), "")
            print(f"[{self.labels[uid]}] 写 FC{fc:02d} {'线圈' if kind == 1 else '寄存器'}"
                  f"[{addr + i}] = {v}  {name}")

    def process(self, uid: int, pdu: bytes) -> bytes | None:
        unit = self.units.get(uid)
        if unit is None:
            return None  # 未配置的从站号：不应答，与真实设备一致
        fc = pdu[0]

        def exc(code: int) -> bytes:
            return bytes([fc | 0x80, code])

        try:
            if fc in (1, 2, 3, 4):
                addr, qty = struct.unpack(">HH", pdu[1:5])
                if not 1 <= qty <= (2000 if fc <= 2 else 125):
                    return exc(3)
                table = unit.regs[fc]
                if fc <= 2:
                    data = bytearray((qty + 7) // 8)
                    for i in range(qty):
                        if table.get(addr + i, 0):
                            data[i // 8] |= 1 << (i % 8)
                    return bytes([fc, len(data)]) + bytes(data)
                words = [table.get(addr + i, 0) for i in range(qty)]
                return bytes([fc, qty * 2]) + struct.pack(f">{qty}H", *words)
            if fc == 5:
                addr, val = struct.unpack(">HH", pdu[1:5])
                if val not in (0, 0xFF00):
                    return exc(3)
                unit.regs[1][addr] = 1 if val else 0
                self.log_write(uid, fc, addr, [1 if val else 0])
                return pdu[:5]
            if fc == 6:
                addr, val = struct.unpack(">HH", pdu[1:5])
                unit.regs[3][addr] = val
                self.log_write(uid, fc, addr, [val])
                return pdu[:5]
            if fc == 15:
                addr, qty, nbytes = struct.unpack(">HHB", pdu[1:6])
                if not 1 <= qty <= 1968 or nbytes != (qty + 7) // 8 or len(pdu) < 6 + nbytes:
                    return exc(3)
                bits = [(pdu[6 + i // 8] >> (i % 8)) & 1 for i in range(qty)]
                for i, b in enumerate(bits):
                    unit.regs[1][addr + i] = b
                self.log_write(uid, fc, addr, bits)
                return pdu[:5]
            if fc == 16:
                addr, qty, nbytes = struct.unpack(">HHB", pdu[1:6])
                if not 1 <= qty <= 123 or nbytes != qty * 2 or len(pdu) < 6 + nbytes:
                    return exc(3)
                words = list(struct.unpack(f">{qty}H", pdu[6:6 + nbytes]))
                for i, w in enumerate(words):
                    unit.regs[3][addr + i] = w
                self.log_write(uid, fc, addr, words)
                return pdu[:5]
            return exc(1)
        except struct.error:
            return exc(3)

    async def handle(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            while True:
                tid, proto, length, uid = struct.unpack(">HHHB", await reader.readexactly(7))
                pdu = await reader.readexactly(length - 1)
                if proto != 0 or not pdu:
                    break
                resp = self.process(uid, pdu)
                if resp is not None:
                    writer.write(struct.pack(">HHHB", tid, 0, len(resp) + 1, uid) + resp)
                    await writer.drain()
        except (asyncio.IncompleteReadError, ConnectionError):
            pass
        finally:
            writer.close()


def parse_sets(items: list[str]) -> dict[str, dict[str, float]]:
    result: dict[str, dict[str, float]] = {}
    for item in items:
        try:
            left, value = item.split("=", 1)
            dev, key = left.split(":", 1)
            result.setdefault(dev, {})[key] = float(value)
        except ValueError:
            raise SystemExit(f"--set 格式应为 设备id:键=工程值，收到: {item}")
    return result


def build(args: argparse.Namespace) -> tuple[list[ModbusDevice], dict[str, tuple[Unit, list[Point]]]]:
    config_path = Path(args.config).resolve()
    root = Path(args.root).resolve() if args.root else config_path.parent.parent
    devices = json.loads(config_path.read_text(encoding="utf-8")).get("devices", {})
    overrides = parse_sets(args.set)
    servers: dict[int, ModbusDevice] = {}
    built: dict[str, tuple[Unit, list[Point]]] = {}

    for dev_id, dev in devices.items():
        cfg = dev.get("config", {})
        if args.devices and dev_id not in args.devices:
            continue
        if cfg.get("com_type") != "ModbusTCP":
            print(f"跳过 {dev_id}: com_type={cfg.get('com_type')}（仅模拟 ModbusTCP）")
            continue
        reg_file = Path(cfg.get("register_file") or "")
        candidate = reg_file if reg_file.is_absolute() else (root / reg_file)
        if not candidate.exists():
            raise SystemExit(f"{dev_id}: 点表不存在 {candidate}")
        port, slave = int(cfg["port"]), int(cfg.get("slave") or 1)
        points = load_points(candidate, dev_id)
        unit = Unit(points=points)
        for p in points:
            for i in range(p.qty):
                unit.names[(RTYPE_FC[p.rtype], p.addr + i)] = f"{p.name}({p.key})"
            fill(unit, p, time.time(), overrides.get(dev_id, {}))
        server = servers.setdefault(port, ModbusDevice(args.bind, port, args.verbose))
        if slave in server.units:
            raise SystemExit(f"{dev_id}: 端口 {port} 的从站号 {slave} 与其他设备冲突")
        server.units[slave] = unit
        server.labels[slave] = dev_id
        built[dev_id] = (unit, points)
        counts = ", ".join(f"{s}{sum(1 for p in points if p.sheet == s)}" for s in SHEETS)
        print(f"{dev_id}: 端口 {port} 从站 {slave} 点表 {candidate.name}（{counts}）")
    if not built:
        raise SystemExit("没有可模拟的 ModbusTCP 设备（检查 config.json 的 devices 或 --devices）")
    return list(servers.values()), built


async def run(servers: list[ModbusDevice], built, overrides, tick: float) -> None:
    running = [await asyncio.start_server(s.handle, s.bind, s.port) for s in servers]
    for s in servers:
        print(f"监听 {s.bind}:{s.port}  从站: {sorted(s.units)}")
    print("Ctrl+C 退出")

    async def updater() -> None:
        while True:
            await asyncio.sleep(tick)
            now = time.time()
            for dev_id, (unit, points) in built.items():
                for p in points:
                    if p.dynamic or p.key in overrides.get(dev_id, {}):
                        if p.sheet == "遥测":
                            fill(unit, p, now, overrides.get(dev_id, {}))

    task = asyncio.create_task(updater())
    try:
        await asyncio.gather(*(s.serve_forever() for s in running))
    finally:
        task.cancel()


def main() -> int:
    ap = argparse.ArgumentParser(description="为 config.json 的 ModbusTCP 设备提供模拟数据")
    ap.add_argument("--config", default="config/config.json")
    ap.add_argument("--root", help="点表相对路径的基准目录（默认 config.json 上一级，即项目根）")
    ap.add_argument("--devices", nargs="*", help="只模拟这些设备 id（默认全部 ModbusTCP 设备）")
    ap.add_argument("--bind", default="0.0.0.0", help="监听地址（默认 0.0.0.0）")
    ap.add_argument("--tick", type=float, default=1.0, help="遥测波动刷新周期秒（默认 1）")
    ap.add_argument("--set", nargs="*", default=[], metavar="DEV:KEY=VAL", help="固定某遥测点的工程值")
    ap.add_argument("--seed", type=int, help="随机种子，便于复现")
    ap.add_argument("--check", action="store_true", help="只解析点表并打印概要，不监听")
    ap.add_argument("-v", "--verbose", action="store_true", help="打印写入（遥调/遥控下发）")
    args = ap.parse_args()
    sys.stdout.reconfigure(line_buffering=True)  # 重定向到文件时也能实时看到日志
    if args.seed is not None:
        random.seed(args.seed)
    try:
        servers, built = build(args)
        if args.check:
            return 0
        asyncio.run(run(servers, built, parse_sets(args.set), args.tick))
    except KeyboardInterrupt:
        pass
    except OSError as exc:
        print(f"启动失败: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
