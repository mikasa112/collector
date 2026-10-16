#!/usr/bin/env python3
"""从 IEC 61850 的 SCL 文件(.icd / .cid / .scd)生成采集器点表初稿(.xlsx)。

只用 Python 标准库, 不依赖 pandas/openpyxl。

    python3 scripts/icd_to_iec61850_points.py 设备.icd -o 点表.xlsx
    python3 scripts/icd_to_iec61850_points.py 设备.scd -o 点表.xlsx --ied PCS0001 --with-quality

生成的是**初稿**, 需要人工审核: key 是按 LN/DO 名自动拼的, name 来自 ICD 里的 desc,
scale/offset/unit/status_words/warn_bits/level 都留空。

输出的 xlsx 包含三个 sheet:

``iec61850``  点表, 第 1 行表头, 列(从 0 开始)依次为
    0 id            点位 id, 全局唯一。按类型分段: yx 1000+, yc 2000+, yk 3000+, yt 4000+,
                    yq 5000+, ytime 6000+, 段内按 ICD 里数据集的顺序递增
    1 key           业务标识, 自动生成: `<LN名>_<DO路径>`, 控制点加 `_cmd`, 品质 `_q`, 时标 `_t`
    2 name          中文名, 来自 ICD 的 DOI desc; 控制点的回读点加 `-回读`
    3 ref           对象引用。监视点指向叶子数据属性 `LD/LN.DO.DA`, 控制点指向控制对象 `LD/LN.DO`
    4 fc            功能约束(ST/MX/SP/...), 控制点留空
    5 type          yx 遥信 | yc 遥测 | yq 品质 | ytime 时标 | yk 遥控(布尔) | yt 遥调(数值)
    6 scale 7 offset 8 unit    留空, 人工填
    9 enable        1 启用 / 0 禁用。名称含危险关键字的控制点、品质(yq)和时标(ytime)点默认 0
    10 trans 11 status_words 12 warn_bits 13 level    留空, 人工填
    14 ctl_opts     控制点的选项(interlock,synchro,test), 留空
    15 feedback     控制点的回读点 key。命令应答成功不代表设备已执行(direct-normal 模式下服务端
                    检查通过就回成功), 要靠回读点确认, 所以每个控制点都应有回读点

``source``    取数来源, 每个 ReportControl 一行: kind,ref,dataset,trg,intg_pd,buf_tm,interval,gi,purge,enable。
              RptEnabled max > 1 的 RCB 在设备上有多个实例(name01..nn), ref 末尾带 `*` 表示通配

``icd_info``  ICD 的来源信息和通讯参数(IP、AP-Title、选择器), 采集器读取点表时会忽略这个 sheet
"""

from __future__ import annotations

import argparse
import hashlib
import sys
import zipfile
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable, Iterator
from xml.sax.saxutils import escape

POINT_HEADER = [
    "id", "key", "name", "ref", "fc", "type", "scale", "offset", "unit", "enable",
    "trans", "status_words", "warn_bits", "level", "ctl_opts", "feedback",
]
SOURCE_HEADER = ["kind", "ref", "dataset", "trg", "intg_pd", "buf_tm", "interval", "gi", "purge", "enable"]
ID_BASE = {"yx": 1000, "yc": 2000, "yk": 3000, "yt": 4000, "yq": 5000, "ytime": 6000}

# 描述里含这些词的控制点默认禁用: 误操作后果严重, 需要人工确认后再启用。
DEFAULT_DANGER_KEYWORDS = ("恢复出厂", "复位", "急停", "重启", "对时", "系统时间")

# 各 CDC 里"代表值"的属性路径(后缀匹配, 从长到短)。
PRIMARY_SUFFIXES: list[tuple[tuple[str, ...], str]] = sorted(
    [
        (("stVal",), "yx"),          # SPS DPS INS ENS SPC DPC INC ENC
        (("general",), "yx"),        # ACT ACD
        (("mag", "f"), "yc"),        # MV
        (("mag", "i"), "yc"),
        (("instMag", "f"), "yc"),    # SAV
        (("instMag", "i"), "yc"),
        (("mxVal", "f"), "yc"),      # APC 的回读
        (("mxVal", "i"), "yc"),
        (("cVal", "mag", "f"), "yc"),  # CMV(WYE/DEL 的各相)
        (("cVal", "mag", "i"), "yc"),
        (("actVal",), "yc"),         # BCR
    ],
    key=lambda x: -len(x[0]),
)

# 控制对象里表示"可控"的属性。
OPER_NAMES = ("Oper", "SBOw")
# 这些基本类型无法放进采集器的数值点, 直接跳过。
UNSUPPORTED_BTYPES = {"VisString32", "VisString64", "VisString65", "VisString129", "VisString255",
                      "Unicode255", "Octet64", "Octet6", "Octet8", "EntryTime", "Check", "ObjRef", "Currency"}
MAX_DEPTH = 16


# ----------------------------------------------------------------------------
# SCL 解析
# ----------------------------------------------------------------------------

@dataclass
class DA:
    name: str
    btype: str
    type_id: str | None
    fc: str | None
    val: str | None


@dataclass
class DOType:
    cdc: str
    das: dict[str, DA]
    sdos: dict[str, str]


@dataclass
class Leaf:
    path: tuple[str, ...]
    fc: str | None
    btype: str


class Types:
    """DataTypeTemplates 里的类型表。"""

    def __init__(self, root: ET.Element):
        dtt = root.find("DataTypeTemplates")
        if dtt is None:
            dtt = ET.Element("DataTypeTemplates")
        self.lntypes: dict[str, dict[str, str]] = {
            t.get("id"): {d.get("name"): d.get("type") for d in t.findall("DO")}
            for t in dtt.findall("LNodeType")
        }
        self.dotypes: dict[str, DOType] = {}
        for t in dtt.findall("DOType"):
            das = {}
            for da in t.findall("DA"):
                val = da.find("Val")
                das[da.get("name")] = DA(
                    da.get("name"), da.get("bType"), da.get("type"), da.get("fc"),
                    val.text.strip() if val is not None and val.text else None,
                )
            self.dotypes[t.get("id")] = DOType(
                t.get("cdc"), das, {s.get("name"): s.get("type") for s in t.findall("SDO")}
            )
        self.datypes: dict[str, dict[str, tuple[str, str | None]]] = {
            t.get("id"): {b.get("name"): (b.get("bType"), b.get("type")) for b in t.findall("BDA")}
            for t in dtt.findall("DAType")
        }

    def struct_leaves(self, type_id, prefix, fc, warn, depth=0) -> Iterator[Leaf]:
        bdas = self.datypes.get(type_id)
        if bdas is None or depth > MAX_DEPTH:
            warn(f"找不到 DAType {type_id} (或嵌套过深)")
            return
        for name, (btype, tid) in bdas.items():
            path = prefix + (name,)
            if btype == "Struct":
                yield from self.struct_leaves(tid, path, fc, warn, depth + 1)
            else:
                yield Leaf(path, fc, btype)

    def do_leaves(self, dotype_id, fc, warn, prefix=(), depth=0) -> Iterator[Leaf]:
        """一个 DO 下(含 SDO 递归)所有 FC 匹配的叶子数据属性, 路径相对于该 DO。"""
        dt = self.dotypes.get(dotype_id)
        if dt is None or depth > MAX_DEPTH:
            warn(f"找不到 DOType {dotype_id} (或嵌套过深)")
            return
        for da in dt.das.values():
            if fc and da.fc != fc:
                continue
            path = prefix + (da.name,)
            if da.btype == "Struct":
                yield from self.struct_leaves(da.type_id, path, da.fc, warn)
            else:
                yield Leaf(path, da.fc, da.btype)
        for sdo, tid in dt.sdos.items():
            yield from self.do_leaves(tid, fc, warn, prefix + (sdo,), depth + 1)

    def resolve_do(self, lntype_id: str, do_path: tuple[str, ...]) -> str | None:
        tid = self.lntypes.get(lntype_id, {}).get(do_path[0])
        for sdo in do_path[1:]:
            if tid is None:
                return None
            tid = self.dotypes.get(tid, DOType("", {}, {})).sdos.get(sdo)
        return tid


@dataclass
class DOI:
    desc: str
    ctl_model: str | None


@dataclass
class LN:
    ld: str            # 点表 ref 里的 LD 名(ldName 或 inst)
    name: str          # 含前缀的 LN 名, 如 staGGIO1; LN0 为 LLN0
    lntype: str
    dois: dict[str, DOI]
    element: ET.Element


@dataclass
class ReportControl:
    ln: LN
    name: str
    data_set: str
    buffered: bool
    intg_pd: str | None
    buf_time: str | None
    trg: list[str]
    gi: bool
    max_instances: int


def load_scl(path: Path) -> ET.Element:
    root = ET.parse(path).getroot()
    for el in root.iter():
        if isinstance(el.tag, str) and "}" in el.tag:
            el.tag = el.tag.split("}", 1)[1]
    return root


def select_ied(root: ET.Element, ied_name: str | None, ap_name: str | None):
    ieds = root.findall("IED")
    if ied_name:
        ieds = [i for i in ieds if i.get("name") == ied_name]
    if not ieds:
        raise SystemExit(f"找不到 IED {ied_name!r}")
    if len(ieds) > 1:
        names = ", ".join(i.get("name") for i in ieds)
        raise SystemExit(f"文件里有多个 IED({names}), 请用 --ied 指定")
    ied = ieds[0]
    aps = [a for a in ied.findall("AccessPoint") if a.find("Server") is not None]
    if ap_name:
        aps = [a for a in aps if a.get("name") == ap_name]
    if not aps:
        raise SystemExit(f"IED {ied.get('name')} 下找不到带 Server 的 AccessPoint")
    if len(aps) > 1:
        names = ", ".join(a.get("name") for a in aps)
        raise SystemExit(f"IED 下有多个 AccessPoint({names}), 请用 --ap 指定")
    return ied, aps[0]


def parse_lns(server: ET.Element) -> list[LN]:
    out = []
    for ld in server.findall("LDevice"):
        ld_ref = ld.get("ldName") or ld.get("inst")
        for el in [ld.find("LN0")] + ld.findall("LN"):
            if el is None:
                continue
            name = (el.get("prefix") or "") + el.get("lnClass") + (el.get("inst") or "")
            dois = {}
            for doi in el.findall("DOI"):
                ctl = None
                for dai in doi.findall("DAI"):
                    if dai.get("name") == "ctlModel":
                        v = dai.find("Val")
                        ctl = v.text.strip() if v is not None and v.text else None
                dois[doi.get("name")] = DOI(doi.get("desc") or "", ctl)
            out.append(LN(ld_ref, name, el.get("lnType"), dois, el))
    return out


def parse_report_controls(ln: LN) -> list[ReportControl]:
    out = []
    for rc in ln.element.findall("ReportControl"):
        trg_el = rc.find("TrgOps")
        trg, gi = [], True
        if trg_el is not None:
            trg = [k for k in ("dchg", "qchg", "dupd", "period") if trg_el.get(k) == "true"]
            gi = trg_el.get("gi", "true") != "false"
        rpt = rc.find("RptEnabled")
        out.append(ReportControl(
            ln, rc.get("name"), rc.get("datSet"), rc.get("buffered") == "true",
            rc.get("intgPd"), rc.get("bufTime"), trg, gi,
            int(rpt.get("max", "1")) if rpt is not None else 1,
        ))
    return out


# ----------------------------------------------------------------------------
# 点的生成
# ----------------------------------------------------------------------------

@dataclass
class Point:
    kind: str                       # yx yc yq ytime yk yt
    ld: str
    ln: str
    path: tuple[str, ...]           # LN 之下的完整路径(控制点只到 DO)
    node: tuple[str, ...]           # DO(含 SDO)路径, 用来取 desc 和拼 key
    fc: str
    role: str                       # value | quality | time | control
    desc: str = ""
    datasets: set[str] = field(default_factory=set)
    is_feedback: bool = False
    ctl_model: str = ""
    enable: int = 1
    disabled_reason: str = ""
    pid: int = 0
    key: str = ""
    name: str = ""
    feedback_key: str = ""

    @property
    def ref(self) -> str:
        return f"{self.ld}/{self.ln}." + ".".join(self.path)

    @property
    def node_ref(self) -> str:
        return f"{self.ln}." + ".".join(self.node)


def classify(path: tuple[str, ...], btype: str):
    """返回 (role, kind, DO 节点路径长度) 或 None。"""
    if path[-1] == "q" and btype == "Quality":
        return "quality", "yq", len(path) - 1
    if path[-1] == "t" and btype == "Timestamp":
        return "time", "ytime", len(path) - 1
    for suffix, kind in PRIMARY_SUFFIXES:
        if len(path) > len(suffix) and path[-len(suffix):] == suffix:
            return "value", kind, len(path) - len(suffix)
    return None


def fallback_kind(btype: str) -> str | None:
    """数据集里直接点名的叶子(daName), 不是常见代表值时按基本类型归类。"""
    if btype in UNSUPPORTED_BTYPES:
        return None
    if btype in ("BOOLEAN", "Dbpos", "Enum", "Tcmd", "INT8", "INT16", "INT32", "INT8U", "INT16U", "INT24U", "INT32U"):
        return "yx"
    if btype in ("FLOAT32", "FLOAT64", "INT64", "INT64U"):
        return "yc"
    return None


class Builder:
    def __init__(self, root, ied, ap, danger_keywords, with_quality):
        self.root, self.ied, self.ap = root, ied, ap
        self.types = Types(root)
        self.lns = parse_lns(ap.find("Server"))
        self.ln_index = {(ln.ld, ln.name): ln for ln in self.lns}
        # ref 里的 LD 可能是 ldName; FCDA 里写的是 inst, 两者都能查
        for ld in ap.find("Server").findall("LDevice"):
            for ln in self.lns:
                if ln.ld == (ld.get("ldName") or ld.get("inst")):
                    self.ln_index[(ld.get("inst"), ln.name)] = ln
        self.multi_ld = len(ap.find("Server").findall("LDevice")) > 1
        self.danger = tuple(danger_keywords)
        self.with_quality = with_quality
        self.warnings: list[str] = []
        self.points: dict[tuple, Point] = {}
        self.report_controls: list[ReportControl] = []
        self.datasets: dict[tuple[str, str], list[str]] = {}   # (ld, 数据集名) -> 成员描述
        self.dataset_sizes: dict[str, int] = {}

    def warn(self, msg: str):
        self.warnings.append(msg)

    # -- 数据集 -> 监视点 ---------------------------------------------------

    def collect_monitoring(self):
        for ln in self.lns:
            self.report_controls += parse_report_controls(ln)
            for ds in ln.element.findall("DataSet"):
                self.dataset_sizes[ds.get("name")] = len(ds.findall("FCDA"))
                for fcda in ds.findall("FCDA"):
                    self.expand_fcda(ds.get("name"), fcda)

    def expand_fcda(self, ds_name: str, fcda: ET.Element):
        ld_inst = fcda.get("ldInst")
        ln_name = (fcda.get("prefix") or "") + (fcda.get("lnClass") or "") + (fcda.get("lnInst") or "")
        ln = self.ln_index.get((ld_inst, ln_name))
        where = f"数据集 {ds_name} 的 {ld_inst}/{ln_name}.{fcda.get('doName')}"
        if ln is None:
            self.warn(f"{where}: 找不到这个 LN, 已跳过")
            return
        if not fcda.get("doName"):
            self.warn(f"{where}: FCDA 没有 doName, 已跳过")
            return
        do_path = tuple(fcda.get("doName").split("."))
        fc = fcda.get("fc")
        tid = self.types.resolve_do(ln.lntype, do_path)
        if tid is None:
            self.warn(f"{where}: 类型里没有这个 DO, 已跳过")
            return

        explicit = fcda.get("daName")
        if explicit:
            leaves = list(self.explicit_leaves(tid, tuple(explicit.split(".")), fc, where))
        else:
            leaves = list(self.types.do_leaves(tid, fc, lambda m: self.warn(f"{where}: {m}")))
        full = [Leaf(do_path + lf.path, fc, lf.btype) for lf in leaves]

        # 同一个节点同时有 f 和 i 时只保留 f
        has_f = {lf.path[:-1] for lf in full if lf.path[-1] == "f"}
        found_value = False
        for lf in full:
            c = classify(lf.path, lf.btype)
            if c is None:
                kind = fallback_kind(lf.btype) if explicit else None
                if kind is None:
                    if explicit:
                        self.warn(f"{where}: 属性 {'.'.join(lf.path)} 的类型 {lf.btype} 不支持, 已跳过")
                    continue
                role, node_len = "value", len(lf.path) - 1
            else:
                role, kind, node_len = c
            if lf.path[-1] == "i" and lf.path[:-1] in has_f:
                continue
            if role == "value":
                found_value = True
            if role in ("quality", "time") and not self.with_quality:
                continue
            self.add_point(ln, lf.path, lf.path[:node_len], fc, kind, role, ds_name)
        if not explicit and not found_value:
            self.warn(f"{where}: 没有识别出代表值的属性(CDC {self.types.dotypes[tid].cdc}), 已跳过")

    def explicit_leaves(self, dotype_id, da_path, fc, where):
        dt = self.types.dotypes.get(dotype_id)
        da = dt.das.get(da_path[0]) if dt else None
        if da is None:
            self.warn(f"{where}: 类型里没有属性 {da_path[0]}, 已跳过")
            return
        btype, tid = da.btype, da.type_id
        for name in da_path[1:]:
            bda = self.types.datypes.get(tid, {}).get(name)
            if bda is None:
                self.warn(f"{where}: 找不到属性 {'.'.join(da_path)}, 已跳过")
                return
            btype, tid = bda
        if btype == "Struct":
            yield from self.types.struct_leaves(tid, da_path, fc, lambda m: self.warn(f"{where}: {m}"))
        else:
            yield Leaf(da_path, fc, btype)

    def add_point(self, ln: LN, path, node, fc, kind, role, ds_name):
        ident = (ln.ld, ln.name, path, fc)
        p = self.points.get(ident)
        if p is None:
            doi = ln.dois.get(node[0])
            p = Point(kind, ln.ld, ln.name, path, node, fc, role, desc=doi.desc if doi else "")
            if role in ("quality", "time"):
                # 数量是值点的两倍, 生成出来供按需启用
                p.enable, p.disabled_reason = 0, "品质/时标默认禁用"
            self.points[ident] = p
        if ds_name:
            p.datasets.add(ds_name)
        return p

    # -- 控制点 -------------------------------------------------------------

    def collect_controls(self):
        self.controls: list[Point] = []
        for ln in self.lns:
            for do_name, tid in self.types.lntypes.get(ln.lntype, {}).items():
                dt = self.types.dotypes.get(tid)
                oper = next((dt.das[n] for n in OPER_NAMES if dt and n in dt.das), None)
                if oper is None or oper.fc != "CO":
                    continue
                doi = ln.dois.get(do_name)
                default = dt.das.get("ctlModel").val if "ctlModel" in dt.das else None
                model = (doi.ctl_model if doi and doi.ctl_model else default) or "status-only"
                if model in ("status-only", "0"):
                    continue
                kind = self.control_kind(dt, oper)
                p = Point(kind, ln.ld, ln.name, (do_name,), (do_name,), "", "control",
                          desc=doi.desc if doi else "", ctl_model=model)
                for kw in self.danger:
                    if kw in p.desc:
                        p.enable, p.disabled_reason = 0, f"名称含 {kw!r}"
                        break
                self.controls.append(p)

    def control_kind(self, dt: DOType, oper: DA) -> str:
        if dt.cdc in ("SPC", "DPC"):
            return "yk"
        if dt.cdc in ("INC", "ENC", "BSC", "APC", "ISC"):
            return "yt"
        ctl_val = self.types.datypes.get(oper.type_id, {}).get("ctlVal")
        return "yk" if ctl_val and ctl_val[0] == "BOOLEAN" else "yt"

    def ensure_feedback(self):
        """每个控制点都要有回读点; 数据集里没有的补一个, 运行时会改为轮询它。"""
        by_node = {}
        for p in self.points.values():
            if p.role == "value":
                by_node.setdefault((p.ld, p.ln, p.node), p)
        for c in self.controls:
            fb = by_node.get((c.ld, c.ln, c.node))
            if fb is None:
                ln = self.ln_index[(c.ld, c.ln)] if (c.ld, c.ln) in self.ln_index else None
                tid = self.types.resolve_do(ln.lntype, c.node) if ln else None
                cand = []
                for fc in ("ST", "MX"):
                    for lf in self.types.do_leaves(tid, fc, lambda m: None):
                        r = classify(c.node + lf.path, lf.btype)
                        if r and r[0] == "value":
                            cand.append((c.node + lf.path, fc, r[1]))
                if not cand:
                    self.warn(f"控制点 {c.ref} 没有可用的回读属性, feedback 留空")
                    continue
                path, fc, kind = cand[0]
                fb = self.add_point(ln, path, c.node, fc, kind, "value", "")
                self.warn(f"控制点 {c.ref} 的回读 {fb.ref}[{fc}] 不在任何数据集里, 已补充为点(运行时会轮询)")
            fb.is_feedback = True
            c.feedback_point = fb  # type: ignore[attr-defined]

    # -- 命名 ---------------------------------------------------------------

    def assign_ids_keys_names(self):
        counters = dict(ID_BASE)
        everything = list(self.points.values()) + self.controls
        # 监视点先按创建顺序编号, 控制点最后, 与 ID_BASE 的分段对应
        for p in everything:
            counters[p.kind] += 1
            p.pid = counters[p.kind]

        used_keys: set[str] = set()
        for p in everything:
            base = f"{p.ln}_" + "_".join(p.node)
            if self.multi_ld:
                base = f"{p.ld.lower()}_{base}"
            if p.role == "control":
                key = base + "_cmd"
            elif p.role == "quality":
                key = base + "_q"
            elif p.role == "time":
                key = base + "_t"
            elif classify(p.path, "") is None and len(p.path) > len(p.node):
                key = base + "_" + "_".join(p.path[len(p.node):])
            else:
                key = base
            key = "".join(ch if ch.isalnum() or ch == "_" else "_" for ch in key)
            unique, n = key, 2
            while unique in used_keys:
                unique, n = f"{key}_{n}", n + 1
            used_keys.add(unique)
            p.key = unique

        # 名称: 控制点优先保留干净的名字(下发时可能按名称查找), 其余重名的补上位置
        used_names: set[str] = set()
        for p in self.controls + list(self.points.values()):
            name = p.desc or ".".join(p.node[:1])
            if len(p.node) > 1:
                name += "-" + ".".join(p.node[1:])
            if p.role == "quality":
                name += "-品质"
            elif p.role == "time":
                name += "-时标"
            if p.is_feedback and p.role == "value":
                name += "-回读"
            if name in used_names:
                name += f"[{p.node_ref}]"
            used_names.add(name)
            p.name = name

        for c in self.controls:
            fb = getattr(c, "feedback_point", None)
            c.feedback_key = fb.key if fb else ""


# ----------------------------------------------------------------------------
# 输出行
# ----------------------------------------------------------------------------

def point_rows(b: Builder) -> list[list]:
    rows = []
    for p in sorted(list(b.points.values()) + b.controls, key=lambda p: p.pid):
        rows.append([
            p.pid, p.key, p.name, p.ref, p.fc or None, p.kind,
            None, None, None, p.enable, None, None, None, None, None, p.feedback_key or None,
        ])
    return rows


def source_rows(b: Builder) -> list[list]:
    rows = []
    for rc in b.report_controls:
        marker = "BR" if rc.buffered else "RP"
        ref = f"{rc.ln.ld}/{rc.ln.name}.{marker}.{rc.name}" + ("*" if rc.max_instances > 1 else "")
        rows.append([
            "brcb" if rc.buffered else "urcb", ref, None, ",".join(rc.trg) or None,
            int(rc.intg_pd) if rc.intg_pd else None, int(rc.buf_time) if rc.buf_time else None,
            None, 1 if rc.gi else 0, 0 if rc.buffered else None, 1,
        ])
    return rows


def info_rows(b: Builder, icd_path: Path) -> list[list]:
    ied, ap = b.ied, b.ap
    rows = [
        ["icd_file", icd_path.name],
        ["icd_sha256", hashlib.sha256(icd_path.read_bytes()).hexdigest()[:16]],
        ["ied", ied.get("name")],
        ["ied_desc", ied.get("desc") or ""],
        ["manufacturer", ied.get("manufacturer") or ""],
        ["access_point", ap.get("name")],
        ["logical_devices", ",".join(ld.get("ldName") or ld.get("inst") for ld in ap.find("Server").findall("LDevice"))],
    ]
    for cap in b.root.iter("ConnectedAP"):
        if cap.get("iedName") == ied.get("name") and cap.get("apName") == ap.get("name"):
            params = {p.get("type"): (p.text or "").strip() for p in cap.findall("Address/P")}
            labels = [
                ("IP", "ip"), ("OSI-AP-Title", "remote_ap_title"), ("OSI-AE-Qualifier", "remote_ae_qualifier"),
                ("OSI-PSEL", "remote_psel"), ("OSI-SSEL", "remote_ssel"), ("OSI-TSEL", "remote_tsel"),
            ]
            rows += [[name, params[t]] for t, name in labels if t in params]
            break
    return rows


# ----------------------------------------------------------------------------
# 最小 xlsx 读写(标准库)
# ----------------------------------------------------------------------------

def _col(i: int) -> str:
    s = ""
    i += 1
    while i:
        i, r = divmod(i - 1, 26)
        s = chr(65 + r) + s
    return s


def _cell(ref: str, value, style: int = 0) -> str:
    s = f' s="{style}"' if style else ""
    if value is None:
        return ""
    if isinstance(value, bool):
        value = int(value)
    if isinstance(value, (int, float)):
        num = int(value) if float(value).is_integer() else value
        return f'<c r="{ref}"{s}><v>{num}</v></c>'
    return f'<c r="{ref}"{s} t="inlineStr"><is><t xml:space="preserve">{escape(str(value))}</t></is></c>'


def _sheet_xml(rows: list[list]) -> str:
    widths = [0] * max((len(r) for r in rows), default=0)
    for r in rows:
        for i, v in enumerate(r):
            if v is not None:
                w = sum(2 if ord(ch) > 127 else 1 for ch in str(v))
                widths[i] = max(widths[i], w)
    cols = "".join(
        f'<col min="{i + 1}" max="{i + 1}" width="{min(max(w + 2, 8), 60)}" customWidth="1"/>'
        for i, w in enumerate(widths)
    )
    body = []
    for ri, r in enumerate(rows, start=1):
        cells = "".join(_cell(f"{_col(ci)}{ri}", v, 1 if ri == 1 else 0) for ci, v in enumerate(r))
        body.append(f'<row r="{ri}">{cells}</row>')
    return (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        '<sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews>'
        f"<cols>{cols}</cols><sheetData>{''.join(body)}</sheetData></worksheet>"
    )


def write_xlsx(path: Path, sheets: list[tuple[str, list[list]]]) -> None:
    """写 xlsx。zip 里的时间戳固定, 同样的内容得到逐字节相同的文件。"""
    n = len(sheets)
    parts = {
        "[Content_Types].xml": (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
            '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
            '<Default Extension="xml" ContentType="application/xml"/>'
            '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
            '<Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>'
            + "".join(
                f'<Override PartName="/xl/worksheets/sheet{i}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
                for i in range(1, n + 1)
            )
            + "</Types>"
        ),
        "_rels/.rels": (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
            "</Relationships>"
        ),
        "xl/workbook.xml": (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
            'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>'
            + "".join(
                f'<sheet name="{escape(name)}" sheetId="{i}" r:id="rId{i}"/>'
                for i, (name, _) in enumerate(sheets, start=1)
            )
            + "</sheets></workbook>"
        ),
        "xl/_rels/workbook.xml.rels": (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
            + "".join(
                f'<Relationship Id="rId{i}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{i}.xml"/>'
                for i in range(1, n + 1)
            )
            + f'<Relationship Id="rId{n + 1}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>'
            "</Relationships>"
        ),
        "xl/styles.xml": (
            '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
            '<styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
            '<fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts>'
            '<fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills>'
            '<borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders>'
            '<cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs>'
            '<cellXfs count="2"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/>'
            '<xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/></cellXfs>'
            "</styleSheet>"
        ),
    }
    for i, (_, rows) in enumerate(sheets, start=1):
        parts[f"xl/worksheets/sheet{i}.xml"] = _sheet_xml(rows)
    with zipfile.ZipFile(path, "w") as z:
        for name, data in parts.items():
            info = zipfile.ZipInfo(name, date_time=(2020, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            z.writestr(info, data.encode("utf-8"))


def read_xlsx(path: Path) -> dict[str, list[list]]:
    """读 xlsx 的所有 sheet(支持内联字符串和共享字符串), 空单元格为 None。"""
    ns = {"m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
    rel_ns = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}id"
    with zipfile.ZipFile(path) as z:
        shared: list[str] = []
        if "xl/sharedStrings.xml" in z.namelist():
            for si in ET.fromstring(z.read("xl/sharedStrings.xml")).findall("m:si", ns):
                shared.append("".join(t.text or "" for t in si.iter("{%s}t" % ns["m"])))
        rels = {
            r.get("Id"): r.get("Target")
            for r in ET.fromstring(z.read("xl/_rels/workbook.xml.rels"))
        }
        out = {}
        for sheet in ET.fromstring(z.read("xl/workbook.xml")).find("m:sheets", ns):
            target = rels[sheet.get(rel_ns)].lstrip("/")
            target = target if target.startswith("xl/") else "xl/" + target
            rows = []
            for row in ET.fromstring(z.read(target)).find("m:sheetData", ns):
                cells: dict[int, object] = {}
                for c in row:
                    letters = "".join(ch for ch in c.get("r") if ch.isalpha())
                    idx = 0
                    for ch in letters:
                        idx = idx * 26 + ord(ch) - 64
                    t = c.get("t")
                    if t == "inlineStr":
                        val = "".join(x.text or "" for x in c.iter("{%s}t" % ns["m"]))
                    else:
                        v = c.find("m:v", ns)
                        if v is None or v.text is None:
                            continue
                        if t == "s":
                            val = shared[int(v.text)]
                        elif t in ("str", "e"):
                            val = v.text
                        else:
                            f = float(v.text)
                            val = int(f) if f.is_integer() else f
                    cells[idx - 1] = val
                width = max(cells) + 1 if cells else 0
                rows.append([cells.get(i) for i in range(width)])
            out[sheet.get("name")] = rows
    return out


# ----------------------------------------------------------------------------
# 主流程
# ----------------------------------------------------------------------------

def build(icd: Path, ied_name=None, ap_name=None, with_quality=False, extra_danger=()) -> tuple[Builder, list[tuple[str, list[list]]]]:
    root = load_scl(icd)
    ied, ap = select_ied(root, ied_name, ap_name)
    b = Builder(root, ied, ap, DEFAULT_DANGER_KEYWORDS + tuple(extra_danger), with_quality)
    b.collect_monitoring()
    b.collect_controls()
    b.ensure_feedback()
    b.assign_ids_keys_names()
    sheets = [
        ("iec61850", [POINT_HEADER] + point_rows(b)),
        ("source", [SOURCE_HEADER] + source_rows(b)),
        ("icd_info", [["item", "value"]] + info_rows(b, icd)),
    ]
    return b, sheets


def print_summary(b: Builder, out: Path) -> None:
    pts = list(b.points.values())
    count = lambda kind, pool: sum(1 for p in pool if p.kind == kind)
    print(f"IED {b.ied.get('name')} ({b.ied.get('desc') or '-'}), AccessPoint {b.ap.get('name')}")
    print(f"已写入 {out}")
    print("点数: " + ", ".join(f"{k} {count(k, pts + b.controls)}" for k in ID_BASE if count(k, pts + b.controls)))

    models: dict[str, int] = {}
    for c in b.controls:
        models[c.ctl_model] = models.get(c.ctl_model, 0) + 1
    print("控制模型: " + ", ".join(f"{m} × {n}" for m, n in models.items()))
    sbo = [m for m in models if m.startswith("sbo")]
    if sbo:
        print("  注意: 含 SBO 的控制点, 下发时会先选择再操作")

    disabled = [c for c in b.controls if not c.enable]
    if disabled:
        print(f"默认禁用的控制点 {len(disabled)} 个(需人工确认后再启用):")
        for c in disabled:
            print(f"  {c.key:<24} {c.desc}  ({c.disabled_reason})")

    covered_rcs = {rc.data_set for rc in b.report_controls}
    print("取数来源:")
    for rc in b.report_controls:
        n = sum(1 for p in pts if rc.data_set in p.datasets)
        inst = f"×{rc.max_instances} 个实例" if rc.max_instances > 1 else ""
        print(f"  {'BRCB' if rc.buffered else 'URCB'} {rc.name} {inst} -> 数据集 {rc.data_set} "
              f"({b.dataset_sizes.get(rc.data_set, '?')} 个成员, 对应 {n} 个点), 触发 {','.join(rc.trg) or '-'}, 完整性周期 {rc.intg_pd or '-'} ms")
    uncovered = [p for p in pts if not (p.datasets & covered_rcs)]
    if uncovered:
        print(f"有 {len(uncovered)} 个点没有被任何 RCB 的数据集覆盖, 运行时只能轮询, 如:")
        for p in uncovered[:5]:
            print(f"  {p.key}  {p.ref}[{p.fc}]")
    if b.warnings:
        print(f"警告 {len(b.warnings)} 条:")
        for w in b.warnings:
            print("  - " + w)


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="从 IEC 61850 SCL 文件生成采集器点表初稿(xlsx)")
    ap.add_argument("icd", type=Path, help=".icd / .cid / .scd 文件")
    ap.add_argument("-o", "--output", type=Path, required=True, help="输出的 .xlsx 路径")
    ap.add_argument("--ied", help="IED 名(文件里有多个 IED 时必须指定)")
    ap.add_argument("--ap", help="AccessPoint 名(IED 下有多个带 Server 的接入点时必须指定)")
    ap.add_argument("--with-quality", action="store_true",
                    help="同时为每个值点生成品质(q)和时标(t)点, 默认禁用(enable=0), 按需启用。数量是值点的 2 倍")
    ap.add_argument("--danger-keyword", action="append", default=[], metavar="词",
                    help=f"追加默认禁用控制点的关键字, 可重复。内置: {', '.join(DEFAULT_DANGER_KEYWORDS)}")
    args = ap.parse_args(argv)

    if not args.icd.is_file():
        print(f"找不到文件 {args.icd}", file=sys.stderr)
        return 1
    b, sheets = build(args.icd, args.ied, args.ap, args.with_quality, args.danger_keyword)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    write_xlsx(args.output, sheets)
    print_summary(b, args.output)
    return 0


if __name__ == "__main__":
    sys.exit(main())
