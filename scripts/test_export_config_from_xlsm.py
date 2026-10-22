"""export_config_from_xlsm.py 的协调控制器成员表测试。运行：python3 -I -m unittest scripts/test_export_config_from_xlsm.py"""
import json
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape

sys.path.insert(0, str(Path(__file__).resolve().parent))
import export_config_from_xlsm as exp  # noqa: E402

COLS = "ABCDEFGHIJKLMNOP"


def sheet_xml(rows):
    def cell(r, c, v):
        ref = f"{COLS[c]}{r}"
        return f'<c r="{ref}" t="inlineStr"><is><t>{escape(str(v))}</t></is></c>'

    body = "".join(
        f'<row r="{ri + 1}">' + "".join(cell(ri + 1, ci, v) for ci, v in enumerate(r)) + "</row>"
        for ri, r in enumerate(rows)
    )
    return (
        '<?xml version="1.0" encoding="UTF-8"?>'
        '<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        f"<sheetData>{body}</sheetData></worksheet>"
    )


def write_workbook(path, sheets):
    names = list(sheets)
    wb = "".join(
        f'<sheet name="{n}" sheetId="{i + 1}" r:id="rId{i + 1}"/>' for i, n in enumerate(names)
    )
    rels = "".join(
        f'<Relationship Id="rId{i + 1}" Type="x" Target="worksheets/sheet{i + 1}.xml"/>'
        for i in range(len(names))
    )
    with zipfile.ZipFile(path, "w") as z:
        z.writestr(
            "xl/workbook.xml",
            '<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
            'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">'
            f"<sheets>{wb}</sheets></workbook>",
        )
        z.writestr(
            "xl/_rels/workbook.xml.rels",
            f'<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>',
        )
        for i, n in enumerate(names):
            z.writestr(f"xl/worksheets/sheet{i + 1}.xml", sheet_xml(sheets[n]))


HEADER = ["dev", "rated_kva", "p_set", "q_set", "p_actual", "q_actual",
          "p_max_charge", "p_max_discharge", "q_upper", "q_lower", "enable"]


def row(dev, **kw):
    base = dict(rated_kva="500", p_set="1", q_set="2", p_actual="3", q_actual="4",
                p_max_charge="5", p_max_discharge="6", q_upper="7", q_lower="8", enable="")
    base.update(kw)
    return [dev] + [base[h] for h in HEADER[1:]]


def rows(d):
    return [dict(zip(HEADER, r)) for r in d]


class MembersTest(unittest.TestCase):
    def test_parse_members(self):
        got = exp.parse_coordinator_members(rows([row("ctrl_a"), row("ctrl_b", rated_kva="250.5", p_set="9.0")]))
        self.assertEqual(got[0], {"dev": "ctrl_a", "rated_kva": 500.0, "p_set": 1, "q_set": 2,
                                  "p_actual": 3, "q_actual": 4, "p_max_charge": 5,
                                  "p_max_discharge": 6, "q_upper": 7, "q_lower": 8})
        self.assertEqual((got[1]["rated_kva"], got[1]["p_set"]), (250.5, 9))
        self.assertIsInstance(got[1]["p_set"], int)

    def test_q_limits_optional(self):
        got = exp.parse_coordinator_members(rows([row("a", q_upper="", q_lower="")]))
        self.assertNotIn("q_upper", got[0])
        self.assertNotIn("q_lower", got[0])

    def test_disabled_row_skipped(self):
        got = exp.parse_coordinator_members(rows([row("a"), row("b", enable="0")]))
        self.assertEqual([m["dev"] for m in got], ["a"])

    def test_errors(self):
        for bad, text in [
            (row("a", p_set=""), "p_set 不能为空"),
            (row("a", p_set="x"), "不是数字"),
            (row("a", p_set="1.5"), "非负整数"),
            (row("a", q_lower="-1"), "非负整数"),
            (row("a", p_max_charge=""), "p_max_charge 不能为空"),
            (row("a", q_lower=""), "同时填或同时留空"),
            (row("a", rated_kva="0"), "大于 0"),
        ]:
            with self.assertRaisesRegex(ValueError, text):
                exp.parse_coordinator_members(rows([bad]))
        with self.assertRaisesRegex(ValueError, "重复"):
            exp.parse_coordinator_members(rows([row("a"), row("a")]))

    def test_end_to_end_export(self):
        with tempfile.TemporaryDirectory() as tmp:
            src, out = Path(tmp) / "c.xlsx", Path(tmp) / "config.json"
            write_workbook(src, {
                "project": [["key", "value"], ["project", "t"]],
                "program": [
                    ["feature", "param", "value", "enable"],
                    ["COORDINATOR", "", "", "1"],
                    ["COORDINATOR", "id", "agc", ""],
                    ["COORDINATOR", "tick_ms", "500", ""],
                    ["COORDINATOR", "p_deadband", "1.5", ""],
                    ["COORDINATOR", "positive_is_discharge", "1", ""],
                ],
                "devices": [["id", "type", "desc"]],
                "config": [["id", "type"]],
                "协调控制器成员": [HEADER, row("ctrl_a"), row("ctrl_b")],
            })
            old = sys.argv
            sys.argv = ["x", str(src), str(out)]
            try:
                self.assertEqual(exp.main(), 0)
            finally:
                sys.argv = old
            co = json.loads(out.read_text(encoding="utf-8"))["program"]["coordinator"]
            self.assertEqual((co["enable"], co["id"], co["tick_ms"], co["p_deadband"]), (True, "agc", 500, 1.5))
            self.assertEqual([m["dev"] for m in co["members"]], ["ctrl_a", "ctrl_b"])
            self.assertIs(co["positive_is_discharge"], True)

    def _export(self, sheets):
        with tempfile.TemporaryDirectory() as tmp:
            src, out = Path(tmp) / "c.xlsx", Path(tmp) / "config.json"
            write_workbook(src, sheets)
            old = sys.argv
            sys.argv = ["x", str(src), str(out)]
            try:
                exp.main()
            finally:
                sys.argv = old
            return json.loads(out.read_text(encoding="utf-8"))

    def _base(self, **extra):
        return {
            "project": [["key", "value"], ["project", "t"]],
            "program": [
                ["feature", "param", "value", "enable"],
                ["COORDINATOR", "members", "{\"dev\": \"x\"}", "1"],
            ],
            "devices": [["id", "type", "desc"]],
            "config": [["id", "type"]],
            **extra,
        }

    def test_members_text_in_program_sheet_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "coordinator_members"):
            self._export(self._base())

    def test_members_sheet_overrides_program_sheet_text(self):
        base = self._base(coordinator_members=[HEADER, row("ctrl_a")])
        base["program"].append(["COORDINATOR", "positive_is_discharge", "true", ""])
        got = self._export(base)
        self.assertEqual([m["dev"] for m in got["program"]["coordinator"]["members"]], ["ctrl_a"])

    def test_sign_convention_required_when_enabled(self):
        base = self._base(coordinator_members=[HEADER, row("a")])
        with self.assertRaisesRegex(ValueError, "positive_is_discharge"):
            self._export(base)
        base["program"].append(["COORDINATOR", "positive_is_discharge", "0", ""])
        got = self._export(base)
        self.assertIs(got["program"]["coordinator"]["positive_is_discharge"], False)
        base["program"][-1] = ["COORDINATOR", "positive_is_discharge", "maybe", ""]
        with self.assertRaisesRegex(ValueError, "只能填"):
            self._export(base)

    def test_no_members_sheet_leaves_config_untouched(self):
        with tempfile.TemporaryDirectory() as tmp:
            src, out = Path(tmp) / "c.xlsx", Path(tmp) / "config.json"
            write_workbook(src, {
                "project": [["key", "value"], ["project", "t"]],
                "program": [["feature", "param", "value", "enable"], ["EMU", "", "", "0"]],
                "devices": [["id", "type", "desc"]],
                "config": [["id", "type"]],
            })
            old = sys.argv
            sys.argv = ["x", str(src), str(out)]
            try:
                exp.main()
            finally:
                sys.argv = old
            self.assertNotIn("coordinator", json.loads(out.read_text(encoding="utf-8"))["program"])


if __name__ == "__main__":
    unittest.main()
