#!/usr/bin/env python3
from __future__ import annotations

import json
import re
import sys
import zipfile
from pathlib import Path
from typing import Any
from xml.etree import ElementTree as ET

NS = {
    "m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
    "r": "http://schemas.openxmlformats.org/officeDocument/2006/relationships",
}


def main() -> int:
    source = (
        Path(sys.argv[1]) if len(sys.argv) > 1 else Path("config/鸿合_配置_v2.0.xlsx")
    )
    target = Path(sys.argv[2]) if len(sys.argv) > 2 else source.with_name("config.json")

    workbook = XlsmWorkbook(source)
    project = parse_project_sheet(workbook.sheet_rows("project"))
    project["program"] = parse_program_sheet(workbook.sheet_rows("program"))
    devices = parse_devices(
        workbook.sheet_rows("devices"),
        workbook.sheet_rows("config"),
    )
    project["devices"] = devices

    mqtt_rows = find_optional_sheet_rows(
        workbook,
        ["mqtt_routes", "mqttRules", "MQTT推送规则"],
    )
    mqtt_routes = parse_mqtt_routes(mqtt_rows)
    if mqtt_routes:
        project["mqtt_routes"] = mqtt_routes

    target.write_text(
        json.dumps(project, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    print(target)
    return 0


class XlsmWorkbook:
    def __init__(self, path: Path) -> None:
        self.path = path
        self.archive = zipfile.ZipFile(path)
        self.shared_strings = self._load_shared_strings()
        self.sheet_targets = self._load_sheet_targets()

    def _load_shared_strings(self) -> list[str]:
        if "xl/sharedStrings.xml" not in self.archive.namelist():
            return []
        root = ET.fromstring(self.archive.read("xl/sharedStrings.xml"))
        strings: list[str] = []
        for item in root.findall("m:si", NS):
            strings.append(
                "".join(node.text or "" for node in item.iterfind(".//m:t", NS))
            )
        return strings

    def _load_sheet_targets(self) -> dict[str, str]:
        workbook_root = ET.fromstring(self.archive.read("xl/workbook.xml"))
        rel_root = ET.fromstring(self.archive.read("xl/_rels/workbook.xml.rels"))
        rel_map = {rel.attrib["Id"]: rel.attrib["Target"] for rel in rel_root}
        targets: dict[str, str] = {}
        sheets = workbook_root.find("m:sheets", NS)
        if sheets is None:
            return targets
        for sheet in sheets:
            name = sheet.attrib["name"]
            rel_id = sheet.attrib[
                "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}id"
            ]
            targets[name] = "xl/" + rel_map[rel_id]
        return targets

    def sheet_rows(self, name: str) -> list[dict[str, str]]:
        target = self.sheet_targets.get(name)
        if target is None:
            raise KeyError(f"missing worksheet: {name}")
        root = ET.fromstring(self.archive.read(target))
        sheet_data = root.find("m:sheetData", NS)
        if sheet_data is None:
            return []

        # 单元格值先按 (行号, 列号) 存入网格，再用合并单元格信息把
        # 只写在左上角单元格的值回填到同一合并区域的其它单元格。
        grid: dict[tuple[int, int], str] = {}
        max_col = 0
        for row in sheet_data.findall("m:row", NS):
            row_num = int(row.attrib.get("r", "1"))
            for cell in row.findall("m:c", NS):
                ref = cell.attrib.get("r", f"A{row_num}")
                col = column_index(ref)
                grid[(row_num, col)] = self._cell_value(cell)
                max_col = max(max_col, col)

        for min_col, min_row, max_col_m, max_row_m in self._load_merges(root):
            anchor = grid.get((min_row, min_col), "")
            for r in range(min_row, max_row_m + 1):
                for c in range(min_col, max_col_m + 1):
                    if (r, c) != (min_row, min_col):
                        grid[(r, c)] = anchor

        raw_rows: list[list[str]] = []
        for row_num in sorted({r for r, _ in grid}):
            values = [grid.get((row_num, c), "") for c in range(1, max_col + 1)]
            if any(cell.strip() for cell in values):
                raw_rows.append(values)

        if not raw_rows:
            return []

        headers = [cell.strip() for cell in raw_rows[0]]
        objects: list[dict[str, str]] = []
        for row in raw_rows[1:]:
            item: dict[str, str] = {}
            for idx, header in enumerate(headers):
                if header:
                    item[header] = row[idx] if idx < len(row) else ""
            objects.append(item)
        return objects

    def _load_merges(self, root: ET.Element) -> list[tuple[int, int, int, int]]:
        merge_cells = root.find("m:mergeCells", NS)
        if merge_cells is None:
            return []
        merges: list[tuple[int, int, int, int]] = []
        for merge in merge_cells.findall("m:mergeCell", NS):
            start_ref, end_ref = merge.attrib["ref"].split(":")
            merges.append(
                (
                    column_index(start_ref),
                    row_index(start_ref),
                    column_index(end_ref),
                    row_index(end_ref),
                )
            )
        return merges

    def _cell_value(self, cell: ET.Element) -> str:
        cell_type = cell.attrib.get("t")
        if cell_type == "inlineStr":
            return "".join(node.text or "" for node in cell.iterfind(".//m:t", NS))

        value = cell.find("m:v", NS)
        if value is None or value.text is None:
            return ""

        text = value.text
        if cell_type == "s":
            return self.shared_strings[int(text)]
        if cell_type == "b":
            return "true" if text == "1" else "false"
        return text


def parse_project_sheet(rows: list[dict[str, str]]) -> dict[str, Any]:
    project: dict[str, Any] = {}
    for row in rows:
        key = row.get("key", "").strip()
        if key:
            project[key] = normalize_scalar(row.get("value", ""))
    return project


def parse_devices(
    device_rows: list[dict[str, str]],
    config_rows: list[dict[str, str]],
) -> dict[str, dict[str, Any]]:
    config_by_id = {
        row["id"].strip(): build_device_config(row)
        for row in config_rows
        if row.get("id", "").strip()
    }

    devices: dict[str, dict[str, Any]] = {}
    for row in device_rows:
        dev_id = row.get("id", "").strip()
        if not dev_id:
            continue
        devices[dev_id] = {
            "id": dev_id,
            "type": empty_to_none(row.get("type", "")),
            "desc": empty_to_none(row.get("desc", "")),
            "config": config_by_id.get(dev_id, {}),
        }
    return devices


def build_device_config(row: dict[str, str]) -> dict[str, Any]:
    field_map = {
        "type": "type",
        "comType": "com_type",
        "registerFile": "register_file",
        "interval": "interval",
        "timeout": "timeout",
        "ip": "ip",
        "port": "port",
        "slave": "slave",
        "serialTty": "serial_tty",
        "baudRate": "baud_rate",
        "dataBits": "data_bits",
        "parity": "parity",
        "stopBits": "stop_bits",
        "interface": "interface",
        "desc": "desc",
        "requestInterval": "request_interval",
        "maxGap": "max_gap",
    }
    return {
        out_key: normalize_scalar(row.get(src_key, ""))
        for src_key, out_key in field_map.items()
        if src_key in row
    }


def parse_program_sheet(rows: list[dict[str, str]]) -> dict[str, Any]:
    features: dict[str, dict[str, Any]] = {}
    order: list[str] = []

    for row in rows:
        feature = row.get("feature", "").strip()
        if not feature:
            continue
        key = sanitize_identifier(feature)
        if key not in features:
            features[key] = {}
            order.append(key)
        entry = features[key]

        enable = row.get("enable", "").strip()
        if enable:
            entry["enable"] = normalize_scalar(enable)

        param = row.get("param", "").strip()
        if param:
            entry[param] = normalize_scalar(row.get("value", ""))

    return {key: features[key] for key in order}


def sanitize_identifier(text: str) -> str:
    return re.sub(r"[^0-9a-zA-Z]+", "_", text.strip()).strip("_").lower()


def find_optional_sheet_rows(
    workbook: XlsmWorkbook, names: list[str]
) -> list[dict[str, str]]:
    for name in names:
        if name in workbook.sheet_targets:
            return workbook.sheet_rows(name)
    return []


def parse_mqtt_routes(rows: list[dict[str, str]]) -> list[dict[str, Any]]:
    grouped: dict[str, dict[str, Any]] = {}
    order: list[str] = []

    for row in rows:
        if not is_enabled(row):
            continue

        route = build_mqtt_route_rule(row)
        if route is None:
            continue

        device_id = first_non_empty(row, ["device_id", "deviceId", "设备ID"])
        if not device_id:
            raise ValueError("mqtt route row requires device_id")

        group_key = f"deviceId:{device_id}"
        if group_key not in grouped:
            grouped[group_key] = {
                "device_id": device_id,
                "rules": [],
            }
            order.append(group_key)
        grouped[group_key]["rules"].append(route)

    return [grouped[key] for key in order]


def build_mqtt_route_rule(row: dict[str, str]) -> dict[str, Any] | None:
    topic = first_non_empty(row, ["topic", "topicTemplate", "Topic模板"])
    if not topic:
        return None

    return {
        "topic": topic,
        "point_ids": parse_int_list(
            first_non_empty(row, ["point_ids", "pointIds", "点号列表"])
        ),
    }


def normalize_scalar(value: str) -> Any:
    text = value.strip()
    if text == "":
        return None
    if text.lower() == "true":
        return True
    if text.lower() == "false":
        return False
    if re.fullmatch(r"-?\d+", text):
        return int(text)
    if re.fullmatch(r"-?\d+\.\d+", text):
        return float(text)
    return text


def empty_to_none(value: str) -> str | None:
    text = value.strip()
    return text or None


def first_non_empty(row: dict[str, str], keys: list[str]) -> str:
    for key in keys:
        value = row.get(key, "").strip()
        if value:
            return value
    return ""


def is_enabled(row: dict[str, str]) -> bool:
    value = first_non_empty(row, ["enable", "enabled", "启用"])
    if not value:
        return True
    return value.lower() not in {"0", "false", "no", "off"}


def parse_int_list(value: str) -> list[int]:
    if not value:
        return []
    result: list[int] = []
    for item in parse_str_list(value):
        result.append(int(item))
    return result


def parse_str_list(value: str) -> list[str]:
    if not value:
        return []
    return [part.strip() for part in value.split(",") if part.strip()]


def column_index(cell_ref: str) -> int:
    letters = re.match(r"[A-Z]+", cell_ref)
    if letters is None:
        return 1
    idx = 0
    for ch in letters.group(0):
        idx = idx * 26 + ord(ch) - ord("A") + 1
    return idx


def row_index(cell_ref: str) -> int:
    digits = re.search(r"\d+", cell_ref)
    return int(digits.group(0)) if digits else 1


if __name__ == "__main__":
    raise SystemExit(main())
