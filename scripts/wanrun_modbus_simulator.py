#!/usr/bin/env python3
"""万润 EMS Modbus TCP 独立模拟器。

该程序不参与 collector 生产进程：它只在 enabled=true 时启动两个本地
Modbus TCP 端点，collector 仍按真实点表完成采集、Lua 映射和 MQTT 上送。
"""

from __future__ import annotations

import argparse
import asyncio
import copy
import json
import math
import signal
import struct
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


SUPPORTED_READ_FUNCTIONS = {1, 2, 3, 4}
SUPPORTED_WRITE_FUNCTIONS = {5, 6, 15, 16}


def _u16(value: int | float) -> int:
    return int(value) & 0xFFFF


def _i32_words(value: int) -> list[int]:
    raw = value & 0xFFFFFFFF
    return [(raw >> 16) & 0xFFFF, raw & 0xFFFF]


def _u32_words(value: int) -> list[int]:
    return [(value >> 16) & 0xFFFF, value & 0xFFFF]


@dataclass
class UnitImage:
    profile: str
    rack: int = 0
    input_registers: dict[int, int] = field(default_factory=dict)
    holding_registers: dict[int, int] = field(default_factory=dict)
    coils: dict[int, bool] = field(default_factory=dict)
    discrete_inputs: dict[int, bool] = field(default_factory=dict)
    started_at: float = field(default_factory=time.monotonic)

    def __post_init__(self) -> None:
        if self.profile == "pcs":
            self._init_pcs()
        elif self.profile == "bau":
            self._init_bau()
        elif self.profile == "bcu":
            self._init_bcu()
        else:
            raise ValueError(f"未知模拟设备类型: {self.profile}")

    def _set_i32(self, address: int, raw_value: int) -> None:
        hi, lo = _i32_words(raw_value)
        self.input_registers[address] = hi
        self.input_registers[address + 1] = lo

    def _set_u32(self, address: int, raw_value: int) -> None:
        hi, lo = _u32_words(raw_value)
        self.input_registers[address] = hi
        self.input_registers[address + 1] = lo

    def _init_pcs(self) -> None:
        # 原始值按 PCS_英博_大机.xlsx 的缩放系数填充。
        values = {
            30000: 2, 30002: 4000, 30004: 3995, 30006: 4005,
            30008: 1800, 30010: 1790, 30012: 1810,
            30014: 12000, 30016: 13300, 30018: 902,
            30020: 325, 30022: 330, 30024: 328,
            30026: 321, 30028: 326, 30030: 324,
            30032: 5000, 30034: 11800, 30036: 1200, 30038: 995,
            30044: 1250, 30046: 1180, 30048: 18400, 30050: 17600,
            30052: 15000, 30054: 14800, 30056: 265, 30058: 550,
            30060: 25000, 30062: 25000, 30064: 15000, 30066: 15000,
            30068: 1000,
        }
        for address, raw in values.items():
            self._set_i32(address, raw)
        self.input_registers.update({
            30070: 1, 30071: 0, 30090: 13300, 30091: 902,
            30092: 650, 30093: 980, 30094: 1800, 30095: 1800,
            30096: 15000, 30097: 10000, 30098: 18000, 30099: 17000,
            30100: 1, 30101: 25000, 30102: 0,
            30103: 3320, 30104: 3260, 30105: 285, 30106: 238,
        })
        for address in range(40000, 40072):
            self.holding_registers[address] = 0

    def _init_bau(self) -> None:
        for address in range(4097, 4350):
            self.input_registers[address] = 0
        self.input_registers.update({
            4136: 1, 4137: 0, 4138: 0x0FFF, 4141: 0, 4142: 0x0FFF,
            4145: 2, 4146: 2, 4148: 0,
            8193: 13300, 8196: 650, 8197: 980, 8198: 500,
            8199: 1800, 8200: 1700, 8201: 1800, 8202: 1800,
            8203: 12, 8204: 18, 8205: 1, 8206: 1, 8207: 8,
            8208: 3320, 8209: 12, 8210: 1, 8211: 401, 8212: 3260,
            8213: 3290, 8214: 5, 8215: 1, 8216: 80, 8217: 285,
            8218: 9, 8219: 1, 8220: 150, 8221: 238, 8222: 261,
            8227: 12, 8228: 12, 8230: 2500, 8231: 2500,
        })
        self._set_i32(8194, 902)
        self._set_u32(8223, 28500)
        self._set_u32(8225, 27300)

    def _init_bcu(self) -> None:
        rack_offset = max(self.rack - 1, 0)
        for address in range(16, 800):
            self.input_registers[address] = 0
        self.input_registers.update({
            16: 0, 20: 0, 25: 2, 26: 0,
            528: 3320 + rack_offset, 529: 8, 530: 1, 531: 8,
            532: 3260 - rack_offset, 533: 401, 534: 13, 535: 17,
            537: 3290, 538: 285 + rack_offset, 539: 80, 540: 3, 541: 12,
            542: 238, 543: 150, 544: 5, 545: 6, 546: 261,
            549: 1800, 550: 1800, 551: 650 - rack_offset, 552: 98,
            561: 1108 + rack_offset, 562: 75 + rack_offset,
            568: 500, 569: 510,
        })
        self._set_u32(588, 2100 + rack_offset * 20)
        self._set_u32(590, 1980 + rack_offset * 20)

        # 每簇 416 个单体电压，原始单位 mV，最终值约 3.26~3.33V。
        for index in range(416):
            wave = int(25 * math.sin((index + rack_offset * 7) / 31.0))
            self.input_registers[5121 + index] = 3290 + wave
        # 每簇 208 个温度点，缩放系数 0.1。
        for index in range(208):
            wave = int(15 * math.sin((index + rack_offset * 3) / 19.0))
            self.input_registers[6145 + index] = 260 + wave

        for address in range(24577, 24721):
            self.holding_registers[address] = 0
        for address in range(1025, 1031):
            self.coils[address] = False

    def refresh_dynamic_values(self) -> None:
        seconds = int(time.monotonic() - self.started_at)
        if self.profile == "pcs":
            self.input_registers[30102] = seconds % 16
            active_kw_raw = 11800 + int(350 * math.sin(seconds / 12.0))
            self._set_i32(30034, active_kw_raw)
        elif self.profile == "bau":
            self.input_registers[4148] = seconds & 0xFFFF
        elif self.profile == "bcu":
            self.input_registers[26] = seconds & 0xFFFF

    def registers(self, function: int) -> dict[int, int]:
        self.refresh_dynamic_values()
        return self.holding_registers if function == 3 else self.input_registers

    def bits(self, function: int) -> dict[int, bool]:
        return self.coils if function == 1 else self.discrete_inputs


class ModbusTcpServer:
    def __init__(self, name: str, host: str, port: int, units: dict[int, UnitImage]):
        self.name = name
        self.host = host
        self.port = port
        self.units = units

    async def start(self) -> asyncio.AbstractServer:
        return await asyncio.start_server(self._handle_client, self.host, self.port)

    async def _handle_client(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        peer = writer.get_extra_info("peername")
        try:
            while True:
                header = await reader.readexactly(7)
                transaction, protocol, length, unit_id = struct.unpack(">HHHB", header)
                if protocol != 0 or length < 2:
                    break
                pdu = await reader.readexactly(length - 1)
                response = self._process(unit_id, pdu)
                writer.write(struct.pack(">HHHB", transaction, 0, len(response) + 1, unit_id) + response)
                await writer.drain()
        except (asyncio.IncompleteReadError, ConnectionResetError, BrokenPipeError):
            pass
        except Exception as exc:  # 模拟器单连接异常不影响其它设备
            print(f"[{self.name}] 客户端 {peer} 处理失败: {exc}", file=sys.stderr)
        finally:
            writer.close()
            await writer.wait_closed()

    def _exception(self, function: int, code: int) -> bytes:
        return bytes([function | 0x80, code])

    def _process(self, unit_id: int, pdu: bytes) -> bytes:
        function = pdu[0] if pdu else 0
        unit = self.units.get(unit_id)
        if unit is None:
            return self._exception(function, 11)
        if function in SUPPORTED_READ_FUNCTIONS:
            if len(pdu) != 5:
                return self._exception(function, 3)
            address, quantity = struct.unpack(">HH", pdu[1:5])
            if quantity < 1 or quantity > (2000 if function <= 2 else 125):
                return self._exception(function, 3)
            if function <= 2:
                source = unit.bits(function)
                packed = bytearray((quantity + 7) // 8)
                for offset in range(quantity):
                    if source.get(address + offset, False):
                        packed[offset // 8] |= 1 << (offset % 8)
                return bytes([function, len(packed)]) + bytes(packed)
            source = unit.registers(function)
            values = [source.get(address + offset, 0) for offset in range(quantity)]
            return bytes([function, quantity * 2]) + struct.pack(f">{quantity}H", *values)
        if function == 5:
            address, raw = struct.unpack(">HH", pdu[1:5])
            unit.coils[address] = raw == 0xFF00
            return pdu[:5]
        if function == 6:
            address, raw = struct.unpack(">HH", pdu[1:5])
            unit.holding_registers[address] = raw
            return pdu[:5]
        if function == 15:
            address, quantity, byte_count = struct.unpack(">HHB", pdu[1:6])
            payload = pdu[6:6 + byte_count]
            for offset in range(quantity):
                unit.coils[address + offset] = bool(payload[offset // 8] & (1 << (offset % 8)))
            return bytes([function]) + struct.pack(">HH", address, quantity)
        if function == 16:
            address, quantity, byte_count = struct.unpack(">HHB", pdu[1:6])
            if byte_count != quantity * 2 or len(pdu) < 6 + byte_count:
                return self._exception(function, 3)
            values = struct.unpack(f">{quantity}H", pdu[6:6 + byte_count])
            for offset, raw in enumerate(values):
                unit.holding_registers[address + offset] = raw
            return bytes([function]) + struct.pack(">HH", address, quantity)
        return self._exception(function, 1)


def load_config(path: Path) -> dict[str, Any]:
    with path.open("r", encoding="utf-8") as handle:
        config = json.load(handle)
    if not isinstance(config.get("enabled"), bool):
        raise ValueError("enabled 必须是 true 或 false")
    servers = config.get("servers")
    if not isinstance(servers, list) or not servers:
        raise ValueError("servers 必须是非空数组")
    return config


def build_servers(config: dict[str, Any]) -> list[ModbusTcpServer]:
    result: list[ModbusTcpServer] = []
    for item in config["servers"]:
        units: dict[int, UnitImage] = {}
        for raw_unit, unit_config in item["units"].items():
            unit_id = int(raw_unit)
            units[unit_id] = UnitImage(
                profile=unit_config["profile"],
                rack=int(unit_config.get("rack", 0)),
            )
        result.append(ModbusTcpServer(item["name"], item["host"], int(item["port"]), units))
    return result


def generate_collector_config(simulator_config: dict[str, Any], simulator_path: Path) -> Path | None:
    generation = simulator_config.get("collector_config")
    if not generation:
        return None
    base = simulator_path.parent
    source = (base / generation["source"]).resolve()
    output = (base / generation["output"]).resolve()
    with source.open("r", encoding="utf-8") as handle:
        collector = json.load(handle)

    targets: dict[str, tuple[str, int]] = {}
    for server in simulator_config["servers"]:
        for device_id in server.get("collector_devices", []):
            targets[device_id] = (server["host"], int(server["port"]))
    for device_id, (host, port) in targets.items():
        device = collector.get("devices", {}).get(device_id)
        if device is None:
            raise ValueError(f"collector 源配置中不存在设备: {device_id}")
        device["config"]["ip"] = host
        device["config"]["port"] = port

    for device_id, register_file in generation.get("register_files", {}).items():
        device = collector.get("devices", {}).get(device_id)
        if device is None:
            raise ValueError(f"collector 源配置中不存在设备: {device_id}")
        device["config"]["register_file"] = register_file

    mqtt_override = generation.get("mqtt")
    if mqtt_override:
        collector["program"]["mqtt"].update(copy.deepcopy(mqtt_override))
    output.parent.mkdir(parents=True, exist_ok=True)
    with output.open("w", encoding="utf-8") as handle:
        json.dump(collector, handle, ensure_ascii=False, indent=2)
        handle.write("\n")
    return output


async def run(config: dict[str, Any]) -> None:
    servers = build_servers(config)
    running = []
    for server in servers:
        instance = await server.start()
        running.append(instance)
        units = ", ".join(str(unit) for unit in sorted(server.units))
        print(f"[{server.name}] 监听 {server.host}:{server.port}，从站: {units}")

    stop = asyncio.Event()
    loop = asyncio.get_running_loop()
    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, stop.set)
        except NotImplementedError:
            pass
    await stop.wait()
    for server in running:
        server.close()
    await asyncio.gather(*(server.wait_closed() for server in running))


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="万润 PCS/BAU/12簇 BCU Modbus TCP 模拟器")
    parser.add_argument("--config", default="config/simulator/wanrun_modbus_simulator.json")
    parser.add_argument("--check", action="store_true", help="只校验配置和生成模拟 collector 配置")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    config_path = Path(args.config).resolve()
    try:
        config = load_config(config_path)
        generated = generate_collector_config(config, config_path)
        print(f"模拟配置校验通过: {config_path}")
        if generated:
            print(f"已生成独立 collector 配置: {generated}")
        if args.check:
            return 0
        if not config["enabled"]:
            print("模拟器未启用（enabled=false），不启动任何 TCP 端口。")
            return 0
        asyncio.run(run(config))
        return 0
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as exc:
        print(f"模拟器启动失败: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
