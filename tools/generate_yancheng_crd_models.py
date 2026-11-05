#!/usr/bin/env python3
"""Generate Salt City CRD backend models and frontend point metadata from the protocol workbook."""

from __future__ import annotations

import json
import re
from pathlib import Path

import openpyxl


ROOT = Path(__file__).resolve().parents[2]
WORKBOOK = ROOT / "collector/config/测控/测控柜_盐城盈海.xlsx"
BACKEND_MODEL = ROOT / "tongsheng-project-tactix/iot-tactix-common/src/main/java/com/inpower/common/model"
FRONTEND_OUTPUT = ROOT / "inpower-B2B-test-platform-web/src/views/remoteControlTest/crd-yancheng-points.json"


def java_name(raw: str, point_id: int) -> str:
    name = re.sub(r"[^0-9A-Za-z_]", "", raw or "point")
    # 源协议 1066/1068 误用了同一个英文标识；按中文含义拆分，避免模型字段重名。
    if name == "xfrBoxMainXfrOverTmpAlmSoe" and point_id == 1068:
        name = "xfrBoxMainXfrHighTmpAlmSoe"
    if name == "xfrBoxRsvd":
        name = f"xfrBoxReserved{point_id}"
    if name and name[0].isdigit():
        name = f"point{name}"
    return name[:1].lower() + name[1:]


def esc(value: object) -> str:
    return str(value or "").replace("\\", "\\\\").replace('"', '\\"')


def read_points() -> dict[str, list[dict[str, object]]]:
    sheet = openpyxl.load_workbook(WORKBOOK, data_only=True, read_only=True)["iec104"]
    result: dict[str, list[dict[str, object]]] = {"yc": [], "yx": [], "yk": [], "yt": []}
    for row in sheet.iter_rows(min_row=2, min_col=1, max_col=10, values_only=True):
        if row[0] is None:
            continue
        point_type = str(row[5] or "").lower()
        if point_type not in result:
            continue
        point_id = int(row[0])
        result[point_type].append(
            {
                "id": point_id,
                "key": java_name(str(row[1] or ""), point_id),
                "label": str(row[2] or row[1] or point_id),
                "unit": str(row[8] or ""),
            }
        )
    return result


def common_header(package: str, class_name: str, description: str) -> list[str]:
    return [
        f"package {package};",
        "",
        "import com.fasterxml.jackson.annotation.JsonIgnoreProperties;",
        "import com.fasterxml.jackson.annotation.JsonProperty;",
        "import com.inpower.common.annotation.Identification;",
        "import lombok.AllArgsConstructor;",
        "import lombok.Builder;",
        "import lombok.Data;",
        "import lombok.NoArgsConstructor;",
        "",
        "/**",
        f" * {description}",
        f" * Generated from {WORKBOOK.name}; do not hand-edit point mappings.",
        " */",
        "@Builder",
        "@AllArgsConstructor",
        "@NoArgsConstructor",
        "@Data",
        "@JsonIgnoreProperties(ignoreUnknown = true)",
        f"public class {class_name} {{",
        "",
    ]


def generate_yc(points: list[dict[str, object]]) -> str:
    lines = common_header("com.inpower.common.model.yc", "CrdYC", "盐城盈海 IEC104 测控柜遥测模型（1~93）。")
    for point in points:
        lines.extend(
            [
                f'    @JsonProperty("{point["id"]}")',
                f'    @Identification(zh = "{esc(point["label"])}", en = "{esc(point["key"])}", unit = "{esc(point["unit"])}")',
                f'    private Float {point["key"]};',
                "",
            ]
        )
    lines.append("}")
    return "\n".join(lines) + "\n"


def is_alarm(label: str) -> bool:
    return any(word in label for word in ("故障", "报警", "告警", "跳闸", "保护", "异常", "事故", "失电", "消失", "烟感"))


def generate_yx(points: list[dict[str, object]]) -> str:
    lines = common_header("com.inpower.common.model.yx", "CrdYX", "盐城盈海 IEC104 测控柜遥信模型（1000~1079，另含65535通讯诊断）。")
    lines.insert(5, "import com.inpower.common.annotation.AlarmBit;")
    for point in points:
        lines.append(f'    @JsonProperty("{point["id"]}")')
        if is_alarm(str(point["label"])):
            lines.append(f'    @AlarmBit(zh = "{esc(point["label"])}", en = "{esc(point["key"])}", level = "1")')
        lines.extend(
            [
                f'    @Identification(zh = "{esc(point["label"])}", en = "{esc(point["key"])}")',
                f'    private Integer {point["key"]};',
                "",
            ]
        )
    lines.extend(
        [
            '    @JsonProperty("65535")',
            '    @AlarmBit(zh = "测控通讯故障诊断", en = "CRD communication fault", level = "1")',
            '    @Identification(zh = "测控通讯故障诊断", en = "CRD communication diagnosis")',
            "    private Integer crdCommDiagnosis;",
            "",
            "}",
        ]
    )
    return "\n".join(lines) + "\n"


def generate_yt(points: list[dict[str, object]]) -> str:
    lines = common_header("com.inpower.common.model.yt", "CrdYT", "盐城盈海 IEC104 测控柜遥控模型（2000~2003）。")
    for point in points:
        lines.extend(
            [
                f'    @JsonProperty("{point["id"]}")',
                f'    @Identification(zh = "{esc(point["label"])}", en = "{esc(point["key"])}", unit = "")',
                f'    private Integer {point["key"]};',
                "",
            ]
        )
    lines.append("}")
    return "\n".join(lines) + "\n"


def frontend_payload(points: dict[str, list[dict[str, object]]]) -> dict[str, object]:
    payload: dict[str, object] = {
        "_doc": "盐城盈海测控点位，由配置_盐城测试平台.xlsx引用的测控柜协议生成。deviceNo固定为0。",
    }
    for point_type in ("yc", "yx", "yk"):
        entries = []
        for point in points[point_type]:
            entry: dict[str, object] = {"id": point["id"], "key": point["key"], "label": point["label"]}
            if point["unit"]:
                entry["unit"] = point["unit"]
            if point_type == "yx" and is_alarm(str(point["label"])):
                entry["alarm"] = True
            if point_type == "yk":
                entry["type"] = "pulse" if point["key"] == "xfrBoxReset" else "switch"
            entries.append(entry)
        payload[point_type] = entries
    payload["yx"].append({"id": 65535, "key": "crdCommDiagnosis", "label": "测控通讯故障诊断", "alarm": True})
    return payload


def main() -> None:
    points = read_points()
    (BACKEND_MODEL / "yc/CrdYC.java").write_text(generate_yc(points["yc"]), encoding="utf-8")
    (BACKEND_MODEL / "yx/CrdYX.java").write_text(generate_yx(points["yx"]), encoding="utf-8")
    (BACKEND_MODEL / "yt/CrdYT.java").write_text(generate_yt(points["yk"]), encoding="utf-8")
    FRONTEND_OUTPUT.write_text(json.dumps(frontend_payload(points), ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"generated yc={len(points['yc'])}, yx={len(points['yx'])}, yk={len(points['yk'])}")


if __name__ == "__main__":
    main()
