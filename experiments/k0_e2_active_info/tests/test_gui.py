import json
import os
os.environ.setdefault("QT_QPA_PLATFORM", "offscreen")
import tempfile
import unittest
from pathlib import Path
from PyQt5 import QtWidgets
from experiments.k0_e2_active_info.gui import E2Dashboard, public_information


class DashboardTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.application = QtWidgets.QApplication.instance() or QtWidgets.QApplication([])

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.artifacts = Path(self.temp.name)
        self.run = self.artifacts / "runs" / "gru64-s0-B0"
        self.run.mkdir(parents=True)
        status = {"run_id": "gru64-s0-B0", "architecture": "gru64", "seed": 0, "arm": "B0",
                  "status": "running", "stage": "imitation", "update": 2, "transitions": 1024,
                  "validation": {"task_success": .75, "call_rate": .2}}
        (self.run / "status.json").write_text(json.dumps(status))
        metrics = [{"update": 1, "validation": {"reward": .5, "task_success": .6, "call_rate": .15}, "steps_per_second": 100},
                   {"update": 2, "validation": {"reward": .7, "task_success": .75, "call_rate": .2}, "steps_per_second": 120}]
        (self.run / "metrics.jsonl").write_text("\n".join(map(json.dumps, metrics)) + '\n{"unfinished":')
        (self.artifacts / "run_summary.json").write_text(json.dumps([{"run_id": "gru64-s0-B0", "scenario_success": {"memory": .9, "language": .8}}]))
        trace = {"representatives": [{"run_id": "gru64-s0-B0", "episodes": [{"episode_id": 3, "success": True, "steps": [
            {"t": 0, "observation": [.2] * 16, "action": 5, "hidden_norm": 1.2, "information": {"category": "取得済み", "latent": "PRIVATE_LATENT"}, "status": "pending", "cost": .05, "resource": .6, "latent": "PRIVATE_LATENT"},
            {"t": 1, "observation": [.7] * 16, "action": 3, "hidden_norm": 1.4, "status": "ok", "language_latency": 0, "cost": 0, "resource": .58}]}]}]}
        (self.artifacts / "j72_results.json").write_text(json.dumps(trace))
        self.window = E2Dashboard(self.artifacts)
        self.window.timer.stop()
        self.application.processEvents()

    def tearDown(self):
        self.window.close()
        self.window.deleteLater()
        self.application.processEvents()
        self.temp.cleanup()

    def test_japanese_dashboard_loads_run_metrics_and_scenarios(self):
        self.assertIn("能動的情報取得", self.window.windowTitle())
        self.assertEqual(self.window.runs_table.rowCount(), 1)
        self.assertEqual(self.window.runs_table.item(0, 4).text(), "学習中")
        x, y = self.window.curves["task_success"].getData()
        self.assertEqual(list(y), [.6, .75])
        self.assertEqual(self.window.scenario_table.item(1, 1).text(), "0.9")
        self.assertEqual(self.window.plot_combo.count(), 10)

    def test_episode_sensor_timeline_and_no_latent_unless_debug(self):
        self.assertEqual(self.window.timeline_table.rowCount(), 2)
        self.assertEqual(self.window.timeline_table.item(0, 1).text(), "言語器官を呼ぶ")
        self.assertEqual(self.window.sensor_table.item(0, 1).text(), "0.2")
        self.assertFalse(self.window.debug_checkbox.isChecked())
        self.assertEqual(self.window.debug_label.text(), "")
        self.assertNotIn("PRIVATE_LATENT", self.window.timeline_table.item(0, 3).text())
        self.assertNotIn("PRIVATE_LATENT", self.window.step_summary.text())
        self.window.debug_checkbox.setChecked(True)
        self.assertIn("PRIVATE_LATENT", self.window.debug_label.text())
        self.window.debug_checkbox.setChecked(False)
        self.assertEqual(self.window.debug_label.text(), "")
        self.window.step_slider.setValue(1)
        self.assertEqual(self.window.sensor_table.item(0, 1).text(), "0.7")
        self.assertIn("正常", self.window.step_summary.text())

    def test_reload_handles_partial_sync_and_updated_status(self):
        (self.run / "status.json").write_text('{"incomplete":')
        self.window.refresh()
        self.assertEqual(self.window.runs_table.rowCount(), 1)
        updated = {"run_id": "gru64-s0-B0", "status": "complete", "validation": {"task_success": .9}}
        (self.run / "status.json").write_text(json.dumps(updated))
        self.window.refresh()
        self.assertEqual(self.window.runs_table.item(0, 4).text(), "完了")
        self.assertIn("完了 1", self.window.summary_label.text())
        self.window.set_artifacts(self.artifacts / "empty")
        self.assertEqual(self.window.runs_table.rowCount(), 0)
        self.assertIn("まだありません", self.window.run_detail.text())

    def test_public_information_recursively_removes_ground_truth(self):
        self.assertEqual(public_information({"source": "J72", "nested": {"target": 1, "latent_state": 0, "confidence": .9}}),
                         {"source": "J72", "nested": {"confidence": .9}})


if __name__ == "__main__":
    unittest.main()
