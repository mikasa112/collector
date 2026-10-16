"""icd_to_iec61850_points.py 的测试: python3 -m unittest scripts/test_icd_to_iec61850_points.py"""

import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
sys.path.insert(0, str(HERE))
import icd_to_iec61850_points as tool  # noqa: E402

PCS_ICD = ROOT / "collector-iec61850/tests/data/INPOWER_MMS_1.5.icd"
PCS_FIXTURE = ROOT / "collector-iec61850/tests/data/pcs_points.xlsx"


def table(rows):
    """sheet 行 -> 以表头为键的 dict 列表(缺的列补 None)。"""
    head = rows[0]
    return [dict(zip(head, r + [None] * (len(head) - len(r)))) for r in rows[1:]]


def generate(icd, tmp, **kw):
    b, sheets = tool.build(icd, **kw)
    out = Path(tmp) / "out.xlsx"
    tool.write_xlsx(out, sheets)
    return b, tool.read_xlsx(out), out


class RealPcsIcd(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.b, cls.data, _ = generate(PCS_ICD, cls.tmp.name)
        cls.rows = table(cls.data["iec61850"])
        cls.by_key = {r["key"]: r for r in cls.rows}

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def count(self, kind):
        return sum(1 for r in self.rows if r["type"] == kind)

    def test_counts_match_icd(self):
        # 22 路遥信 + 6 个遥控的状态回读 / 75 个测量 + 41 个遥调回读 / 6 遥控 / 41 遥调
        self.assertEqual([self.count(k) for k in ("yx", "yc", "yk", "yt")], [28, 116, 6, 41])
        self.assertEqual(self.count("yq") + self.count("ytime"), 0)

    def test_header_and_unique_columns(self):
        self.assertEqual(self.data["iec61850"][0], tool.POINT_HEADER)
        for col in ("id", "key", "name"):
            vals = [r[col] for r in self.rows]
            self.assertEqual(len(vals), len(set(vals)), f"{col} 有重复")

    def test_ids_are_in_their_range(self):
        for r in self.rows:
            base = tool.ID_BASE[r["type"]]
            self.assertTrue(base < r["id"] <= base + 999, r)

    def test_nested_objects_are_flattened_to_leaves(self):
        r = self.by_key["MMXU1_PPV_phsAB"]
        self.assertEqual((r["ref"], r["fc"], r["type"]), ("MONT/MMXU1.PPV.phsAB.cVal.mag.f", "MX", "yc"))
        self.assertEqual(r["name"], "线电压-phsAB")
        r = self.by_key["MMXU1_TotW"]
        self.assertEqual((r["ref"], r["fc"]), ("MONT/MMXU1.TotW.mag.f", "MX"))
        r = self.by_key["staGGIO1_Ind1"]
        self.assertEqual((r["ref"], r["fc"], r["type"]), ("MONT/staGGIO1.Ind1.stVal", "ST", "yx"))

    def test_every_control_has_a_feedback_point(self):
        controls = [r for r in self.rows if r["type"] in ("yk", "yt")]
        self.assertEqual(len(controls), 47)
        for c in controls:
            fb = self.by_key[c["feedback"]]
            self.assertIn(fb["type"], ("yx", "yc"))
            self.assertTrue(fb["ref"].startswith(c["ref"] + "."), (c["ref"], fb["ref"]))
            self.assertIsNone(c["fc"])
            self.assertTrue(fb["name"].endswith("-回读"), fb["name"])
        self.assertEqual(self.by_key["setGAPC1_StrVal6_cmd"]["feedback"], "setGAPC1_StrVal6")
        self.assertEqual(self.by_key["setGAPC1_StrVal6"]["ref"], "MONT/setGAPC1.StrVal6.mxVal.f")

    def test_controls_keep_clean_names_when_names_collide(self):
        # "系统时间（秒）" 同时是控制点、回读点和一个测量点
        self.assertEqual(self.by_key["setGAPC2_StrVal14_cmd"]["name"], "系统时间（秒）")
        self.assertEqual(self.by_key["setGAPC2_StrVal14"]["name"], "系统时间（秒）-回读")
        self.assertEqual(self.by_key["devGGIO2_AnIn9"]["name"], "系统时间（秒）[devGGIO2.AnIn9]")

    def test_dangerous_controls_are_disabled(self):
        disabled = {r["key"] for r in self.rows if r["enable"] == 0}
        self.assertEqual(disabled, {
            "ctlGAPC1_SPCSO2_cmd", "ctlGAPC1_SPCSO3_cmd", "ctlGAPC1_SPCSO4_cmd",
            "setGAPC2_StrVal14_cmd", "setGAPC2_StrVal15_cmd", "setGAPC2_StrVal16_cmd",
            "setGAPC3_StrVal1_cmd", "setGAPC3_StrVal2_cmd", "setGAPC3_StrVal3_cmd",
            "setGAPC3_StrVal4_cmd", "setGAPC3_StrVal6_cmd", "setGAPC3_StrVal7_cmd",
        })
        self.assertEqual(self.by_key["ctlGAPC1_SPCSO1_cmd"]["enable"], 1)

    def test_status_only_objects_are_not_controls(self):
        self.assertFalse(any("Mod" in r["key"] for r in self.rows))

    def test_source_rows(self):
        src = table(self.data["source"])
        self.assertEqual([(r["kind"], r["ref"]) for r in src], [
            ("brcb", "MONT/LLN0.BR.brcbDin*"),
            ("urcb", "MONT/LLN0.RP.urcbAin*"),
            ("urcb", "MONT/LLN0.RP.urcbParameter*"),
        ])
        self.assertEqual([r["intg_pd"] for r in src], [13000, 5000, 15000])
        self.assertTrue(all(r["trg"] == "dchg,period" and r["gi"] == 1 for r in src))
        self.assertEqual([r["purge"] for r in src], [0, None, None])

    def test_info_sheet_has_iso_parameters(self):
        info = dict(self.data["icd_info"][1:])
        self.assertEqual(info["ied"], "PCS0001")
        self.assertEqual(info["ip"], "192.168.1.136")
        self.assertEqual(info["remote_ap_title"], "1 3 9999 106")
        self.assertEqual(info["remote_ae_qualifier"], "33")

    def test_no_warnings_on_the_real_icd(self):
        self.assertEqual(self.b.warnings, [])

    def test_output_is_deterministic(self):
        with tempfile.TemporaryDirectory() as t1, tempfile.TemporaryDirectory() as t2:
            _, _, a = generate(PCS_ICD, t1)
            _, _, b = generate(PCS_ICD, t2)
            self.assertEqual(a.read_bytes(), b.read_bytes())

    def test_committed_fixture_is_up_to_date(self):
        # tests/data/pcs_points.xlsx 是 `--with-quality` 的输出, ICD 或脚本改了要重新生成:
        #   python3 scripts/icd_to_iec61850_points.py collector-iec61850/tests/data/INPOWER_MMS_1.5.icd \
        #       -o collector-iec61850/tests/data/pcs_points.xlsx --with-quality
        with tempfile.TemporaryDirectory() as t:
            _, _, out = generate(PCS_ICD, t, with_quality=True)
            self.assertEqual(out.read_bytes(), PCS_FIXTURE.read_bytes(), "fixture 已过期, 请重新生成")


class WithQuality(unittest.TestCase):
    def test_quality_and_time_rows_are_disabled_by_default(self):
        with tempfile.TemporaryDirectory() as t:
            _, data, _ = generate(PCS_ICD, t, with_quality=True)
        rows = table(data["iec61850"])
        q = [r for r in rows if r["type"] == "yq"]
        tm = [r for r in rows if r["type"] == "ytime"]
        # q/t 按 DO 生成: dsDin 28 个 + dsAin 75 个。dsParameter 的 41 个成员直接点名 mxVal.f,
        # 数据集里没有它们的 q/t, 所以不生成。
        self.assertEqual((len(q), len(tm)), (103, 103))
        self.assertTrue(all(r["enable"] == 0 for r in q + tm))
        r = next(r for r in q if r["key"] == "staGGIO1_Ind1_q")
        self.assertEqual((r["ref"], r["fc"], r["name"]), ("MONT/staGGIO1.Ind1.q", "ST", "工作状态-停机-品质"))
        r = next(r for r in tm if r["key"] == "MMXU1_TotW_t")
        self.assertEqual((r["ref"], r["fc"]), ("MONT/MMXU1.TotW.t", "MX"))


# 一个手写的 SCL: 覆盖真实 ICD 里没有的情况
SYNTHETIC = """<?xml version="1.0"?>
<SCL xmlns="http://www.iec.ch/61850/2003/SCL">
 <IED name="IED1"><AccessPoint name="S1"><Server>
  <LDevice inst="LD0">
   <LN0 lnClass="LLN0" lnType="T_LLN0" inst="">
    <DataSet name="ds1">
     <FCDA ldInst="LD0" lnClass="XCBR" lnInst="1" doName="Pos" fc="ST"/>
     <FCDA ldInst="LD0" lnClass="MMXU" lnInst="1" doName="TotW" fc="MX"/>
     <FCDA ldInst="LD0" lnClass="MMXU" lnInst="1" doName="Cnt" fc="ST"/>
     <FCDA ldInst="LD0" lnClass="MMXU" lnInst="1" doName="Nm" daName="vendor" fc="DC"/>
     <FCDA ldInst="LD0" lnClass="MMXU" lnInst="9" doName="Gone" fc="MX"/>
     <FCDA ldInst="LD0" lnClass="XCBR" lnInst="1" doName="Pos" fc="ST"/>
    </DataSet>
    <ReportControl name="rcb" datSet="ds1" buffered="false" intgPd="2000" bufTime="100">
     <TrgOps dchg="true" gi="false"/><RptEnabled max="1"/>
    </ReportControl>
   </LN0>
   <LN lnClass="XCBR" inst="1" lnType="T_XCBR">
    <DOI name="Pos" desc="断路器"><DAI name="ctlModel"><Val>sbo-with-enhanced-security</Val></DAI></DOI>
   </LN>
   <LN lnClass="MMXU" inst="1" lnType="T_MMXU"/>
  </LDevice>
  <LDevice inst="LD1" ldName="Bay2"><LN0 lnClass="LLN0" lnType="T_LLN0" inst=""/>
   <LN lnClass="MMXU" inst="1" lnType="T_MMXU"/></LDevice>
 </Server></AccessPoint></IED>
 <DataTypeTemplates>
  <LNodeType id="T_LLN0" lnClass="LLN0"/>
  <LNodeType id="T_XCBR" lnClass="XCBR"><DO name="Pos" type="T_DPC"/></LNodeType>
  <LNodeType id="T_MMXU" lnClass="MMXU">
   <DO name="TotW" type="T_MV"/><DO name="Cnt" type="T_INS"/><DO name="Nm" type="T_LPL"/><DO name="Gone" type="T_MV"/>
  </LNodeType>
  <DOType id="T_DPC" cdc="DPC">
   <DA name="Oper" bType="Struct" type="T_Oper" fc="CO"/>
   <DA name="stVal" bType="Dbpos" fc="ST"/><DA name="q" bType="Quality" fc="ST"/><DA name="t" bType="Timestamp" fc="ST"/>
   <DA name="ctlModel" bType="Enum" fc="CF"><Val>status-only</Val></DA>
  </DOType>
  <DOType id="T_MV" cdc="MV">
   <DA name="mag" bType="Struct" type="T_AV" fc="MX"/><DA name="q" bType="Quality" fc="MX"/>
  </DOType>
  <DOType id="T_INS" cdc="INS"><DA name="stVal" bType="INT32" fc="ST"/></DOType>
  <DOType id="T_LPL" cdc="LPL"><DA name="vendor" bType="VisString255" fc="DC"/></DOType>
  <DAType id="T_AV"><BDA name="i" bType="INT32"/><BDA name="f" bType="FLOAT32"/></DAType>
  <DAType id="T_Oper"><BDA name="ctlVal" bType="BOOLEAN"/></DAType>
 </DataTypeTemplates>
</SCL>
"""


class SyntheticScl(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.tmp = tempfile.TemporaryDirectory()
        cls.icd = Path(cls.tmp.name) / "s.icd"
        cls.icd.write_text(SYNTHETIC, encoding="utf-8")
        cls.b, cls.data, _ = generate(cls.icd, cls.tmp.name)
        cls.rows = table(cls.data["iec61850"])
        cls.by_key = {r["key"]: r for r in cls.rows}

    @classmethod
    def tearDownClass(cls):
        cls.tmp.cleanup()

    def test_dbpos_and_integer_values(self):
        self.assertEqual(self.by_key["ld0_XCBR1_Pos"]["ref"], "LD0/XCBR1.Pos.stVal")
        self.assertEqual(self.by_key["ld0_XCBR1_Pos"]["type"], "yx")
        self.assertEqual(self.by_key["ld0_MMXU1_Cnt"]["type"], "yx")

    def test_float_preferred_over_int_when_both_exist(self):
        self.assertEqual(self.by_key["ld0_MMXU1_TotW"]["ref"], "LD0/MMXU1.TotW.mag.f")
        self.assertFalse(any(r["ref"].endswith(".mag.i") for r in self.rows))

    def test_sbo_control_and_its_feedback(self):
        c = self.by_key["ld0_XCBR1_Pos_cmd"]
        self.assertEqual((c["type"], c["ref"], c["feedback"]), ("yk", "LD0/XCBR1.Pos", "ld0_XCBR1_Pos"))
        self.assertEqual(c["enable"], 1)

    def test_duplicate_fcda_yields_one_point(self):
        self.assertEqual(sum(1 for r in self.rows if r["key"] == "ld0_XCBR1_Pos"), 1)

    def test_unsupported_and_dangling_members_warn_and_skip(self):
        joined = "\n".join(self.b.warnings)
        self.assertIn("VisString255", joined)
        self.assertIn("LD0/MMXU9", joined)
        self.assertFalse(any("vendor" in r["ref"] for r in self.rows))

    def test_rcb_without_instances_and_gi_disabled(self):
        (rc,) = table(self.data["source"])
        self.assertEqual(rc["ref"], "LD0/LLN0.RP.rcb")  # max=1: 没有实例后缀, 不加通配
        self.assertEqual((rc["trg"], rc["gi"], rc["intg_pd"], rc["buf_tm"]), ("dchg", 0, 2000, 100))

    def test_ld_name_overrides_inst_in_refs(self):
        # 另一个 LD 里没有任何数据集成员, 只验证 ldName 被识别
        info = dict(self.data["icd_info"][1:])
        self.assertEqual(info["logical_devices"], "LD0,Bay2")

    def test_multiple_lds_prefix_the_keys(self):
        self.assertTrue(self.b.multi_ld)
        self.assertTrue(all(r["key"].startswith("ld0_") for r in self.rows))

    def test_extra_danger_keyword(self):
        with tempfile.TemporaryDirectory() as t:
            _, data, _ = generate(self.icd, t, extra_danger=("断路器",))
        rows = {r["key"]: r for r in table(data["iec61850"])}
        self.assertEqual(rows["ld0_XCBR1_Pos_cmd"]["enable"], 0)


class Errors(unittest.TestCase):
    def test_several_ieds_require_a_choice(self):
        scl = '<SCL xmlns="http://www.iec.ch/61850/2003/SCL"><IED name="A"/><IED name="B"/></SCL>'
        with tempfile.TemporaryDirectory() as t:
            p = Path(t) / "m.scd"
            p.write_text(scl)
            with self.assertRaises(SystemExit) as cm:
                tool.build(p)
            self.assertIn("--ied", str(cm.exception))


class XlsxRoundTrip(unittest.TestCase):
    def test_values_survive(self):
        rows = [["a", "b", "c"], [1, "中文 with  spaces", None], [None, 2.5, "<&>\"'"], [3]]
        with tempfile.TemporaryDirectory() as t:
            p = Path(t) / "x.xlsx"
            tool.write_xlsx(p, [("s1", rows), ("s2", [["x"]])])
            got = tool.read_xlsx(p)
        self.assertEqual(got["s1"], [["a", "b", "c"], [1, "中文 with  spaces"], [None, 2.5, "<&>\"'"], [3]])
        self.assertEqual(got["s2"], [["x"]])


if __name__ == "__main__":
    unittest.main()
