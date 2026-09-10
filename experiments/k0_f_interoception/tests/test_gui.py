"""Missing-data, artifact-schema and read-only GUI regressions."""
import json
import os
from pathlib import Path
import tempfile
import time
import unittest

os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
from PyQt5 import QtWidgets
from experiments.k0_f_interoception.gui import BodyDashboard, read_snapshot, read_jsonl_tail


class DashboardTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.app = QtWidgets.QApplication.instance() or QtWidgets.QApplication([])

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.window = BodyDashboard(self.root, autoload=False)

    def tearDown(self):
        self.window.close()
        self.temp.cleanup()

    def apply(self, **values):
        self.window.apply_snapshot({"root": str(self.root), "errors": [], **values})

    def test_missing_is_not_healthy_zero(self):
        self.apply()
        self.assertIn("未取得", self.window.cards["master"].text())
        self.assertEqual(self.window.frame_table.item(0, 1).text(), "欠損")
        self.assertIn("未判定", self.window.research_label.text())
        self.assertIn("停止済みとは判断しません", self.window.runtime_label.text())
        self.assertEqual(len(self.window.series_plots), 6)
        self.assertTrue(all(not plot.listDataItems() for plot in self.window.series_plots.values()))

    def test_zero_mask_and_invalid_raw_quality_remain_missing(self):
        frame = {"values": [0]*20, "mask": [0]*20, "quality": [0]*20, "age_s": [None]*20, "availability": 0, "source_kind": "real"}
        master = {"timestamp": time.time(), "source_kind": "real", "metrics": {"cpu_temperature_c": 0}, "quality": {"cpu_temperature_c": 0}}
        self.apply(frames=[frame], master=[master])
        self.assertEqual(self.window.frame_table.item(0, 1).text(), "欠損")
        self.assertEqual(self.window.frame_table.item(0, 2).text(), "0")
        self.assertIn("未取得 / 無効", self.window.cards["master"].text())
        self.assertFalse(self.window.series_plots["temperature"].listDataItems())

    def test_valid_zero_is_shown_when_mask_available(self):
        self.apply(frames=[{"values": [0]*20, "mask": [1]*20, "quality": [1]*20, "age_s": [0]*20, "availability": 1}])
        self.assertEqual(self.window.frame_table.item(0, 1).text(), "0")
        self.assertEqual(self.window.frame_table.item(0, 5).text(), "保存時に取得")

    def test_policy_schema_and_provenance(self):
        self.apply(ablation_results={"rows": [{"seed": 1, "architecture": "GRU128", "mode": "BODY", "utility": .7,
                   "deadline_success_rate": .8, "latency_seconds": 1.25}]},
                   core=[{"action": "RUN_P100", "mode": "BODY", "hidden_norm": 2.1, "source_kind": "real_telemetry_measured_cost_replay"}])
        self.assertEqual(self.window.runs_table.rowCount(), 1)
        self.assertEqual(self.window.runs_table.item(0, 2).text(), "BODY")
        self.assertEqual(self.window.runs_table.item(0, 4).text(), "80%")
        self.assertEqual(self.window.runs_table.item(0, 5).text(), "1.25")
        self.assertIn("P100 で実行", self.window.cards["core"].text())
        self.assertIn("実測身体・実測費用の再生評価", self.window.cards["core"].text())

    def test_real_and_synthetic_series_never_merge(self):
        stamp = time.time()
        self.apply(master=[{"timestamp": stamp, "source_kind": kind, "metrics": {"cpu_temperature_c": 40 + index}}
                           for index, kind in enumerate(("real", "synthetic"))])
        curves = self.window.series_plots["temperature"].listDataItems()
        self.assertEqual(len(curves), 2)
        self.assertTrue(any("実測" in curve.name() for curve in curves))
        self.assertTrue(any("合成" in curve.name() for curve in curves))
        self.assertIn("実測", self.window.status_label.text())
        self.assertIn("合成", self.window.status_label.text())

    def test_read_only_and_truncated_stream(self):
        path = self.root / "raw_mac_telemetry.jsonl"
        raw = b'{"source_kind":"real","timestamp":1,"metrics":{}}\n{"unfinished":'
        path.write_bytes(raw)
        result = read_snapshot(self.root)
        self.assertEqual(len(result["mac"]), 1)
        self.assertFalse(result["errors"])
        self.window.apply_snapshot(result)
        self.assertEqual(path.read_bytes(), raw)
        self.assertEqual(list(self.root.iterdir()), [path])

    def test_complete_invalid_line_is_reported(self):
        path = self.root / "raw_master_telemetry.jsonl"
        path.write_bytes(b"bad json\n{}\n")
        records, errors = read_jsonl_tail(path)
        self.assertEqual(records, [{}])
        self.assertEqual(len(errors), 1)

    def test_live_numeric_action_matches_three_resource_contract(self):
        self.apply(core=[{"action": 0, "mode": "BODY", "hidden_norm": 1, "source_kind": "real_new_hardware_job"}],
                   live_results={"rows": [{"seed": 1, "mode": "BODY", "utility": .8, "latency_seconds": .002}]})
        self.assertIn("CPU で実行", self.window.cards["core"].text())
        self.assertNotIn("待機", self.window.cards["core"].text())
        self.assertIn("新規実ジョブ", self.window.cards["core"].text())
        self.assertEqual(self.window.runs_table.item(0, 6).text(), "新規実ジョブ")

    def test_wrong_frame_length_not_silently_normal(self):
        self.apply(frames=[{"values": [1], "mask": [1], "quality": [1], "age_s": [0]}])
        self.assertIn("固定長データ不整合", self.window.frame_summary.text())
        self.assertEqual(self.window.frame_table.item(0, 1).text(), "欠損")


if __name__ == "__main__":
    unittest.main()
