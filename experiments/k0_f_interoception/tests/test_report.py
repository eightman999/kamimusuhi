"""Reports must distinguish actual downstream evidence and never promote missing gates."""
import copy
import json
from pathlib import Path
import tempfile
import unittest

from experiments.k0_f_interoception.report import generate, criterion, best_validation_core


class ReportTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.out = self.root / "report"

    def tearDown(self):
        self.temp.cleanup()

    def write(self, name, value):
        path = self.root / name
        path.write_text("".join(json.dumps(row) + "\n" for row in value) if path.suffix == ".jsonl" else json.dumps(value))

    @staticmethod
    def comparison():
        return dict(n_seeds=8, mean=.2, mean_difference=.2, sd=.02, median=.2, ci95=[.1, .3], exact_sign_p=.0078125, **{"pass": True})

    def fixture(self):
        for name in ("raw_mac_telemetry", "raw_master_telemetry"):
            self.write(name + ".jsonl", [dict(timestamp=100+i, source_kind="real", metrics={}) for i in range(3)])
        self.write("interoceptive_frames.jsonl", [dict(timestamp=100+i, values=[.2]*20, mask=[1]*20, quality=[1]*20, age_s=[0]*20,
                   normalization_identity="frozen", provenance={"master": {"sequence": i}}, availability=1, source_kind="real") for i in range(3)])
        self.write("aligned_body_telemetry.jsonl", [dict(timestamp=100)])
        self.write("normalization_config.json", dict(version="fixed"))
        self.write("training_config.json", dict(identities={"dataset_sha256": "frozen"}, source_commit="abc", seeds=list(range(8))))
        self.write("prediction_probe.json", dict(gate={"pass": True, "relative_mae_improvement": .2}))
        self.write("run_summary.json", dict(rows=[dict(seed=s, training_mode="BODY", architecture="GRU128", best_validation_utility=.8) for s in range(8)]))
        self.write("ablation_results.json", dict(rows=[dict(seed=s, mode=m, training_mode="BODY", architecture="GRU128", utility=.8 if m == "BODY" else .6) for s in range(8) for m in ("BODY", "BLIND", "SHUFFLED", "STALE")],
                   comparisons={"GRU128_BODY_vs_"+m: self.comparison() for m in ("BLIND", "SHUFFLED", "STALE")}))
        self.write("counterfactual_body.json", dict(rows=[dict(seed=s, architecture="GRU128", available=True, action_change_rate=.5) for s in range(8)], comparisons={"GRU128": self.comparison()}))
        self.write("live_results.json", dict(rows=[dict(seed=s, mode=m, utility=.8 if m == "BODY" else .6) for s in range(8) for m in ("BODY", "BLIND", "TRAINED_BLIND")], comparisons={"BODY_vs_"+m: self.comparison() for m in ("BLIND", "TRAINED_BLIND")}))
        self.write("resource_summary.json", dict(telemetry_continuous=True, fixed_frame_valid=True, safety_pass=True, reproducibility_pass=True, execution_complete=True))
        self.write("final_runtime_state.json", dict(cleanup_pass=True))
        self.write("baseline_integrity.json", {"pass": True})

    def test_empty_evidence_is_fail_with_24_sections(self):
        stats, result = generate(self.root, output=self.out)
        self.assertEqual(result["research_status"], "FAIL")
        self.assertFalse(stats["execution_complete"])
        report = (self.out / "K0_F_REPORT.md").read_text()
        self.assertEqual(sum(line.startswith("## ") for line in report.splitlines()), 24)
        self.assertIn("未学習 / 未選定", report)
        self.assertIn("新規実ジョブ", report)

    def test_complete_evidence_can_pass_without_mutating_inputs(self):
        self.fixture()
        before = {path.name: path.read_bytes() for path in self.root.iterdir()}
        stats, result = generate(self.root, output=self.out)
        self.assertEqual(result["research_status"], "PASS")
        self.assertTrue(stats["execution_complete"])
        for name, content in before.items():
            self.assertEqual((self.root / name).read_bytes(), content)

    def test_replay_success_cannot_substitute_for_live_failure(self):
        self.fixture()
        path = self.root / "live_results.json"
        data = json.loads(path.read_text())
        data["comparisons"]["BODY_vs_TRAINED_BLIND"]["ci95"] = [-.1, .2]
        self.write(path.name, data)
        _, result = generate(self.root, output=self.out)
        self.assertEqual(result["research_status"], "FAIL")
        self.assertIn("G_live_BODY_vs_TRAINED_BLIND", result["unmet"])

    def test_missing_or_duplicate_seed_not_eight_independent_trials(self):
        self.fixture()
        path = self.root / "ablation_results.json"
        data = json.loads(path.read_text())
        data["rows"] = [dict(row, seed=0) for row in data["rows"]]
        self.write(path.name, data)
        _, result = generate(self.root, output=self.out)
        self.assertFalse(result["gates"]["C_BODY_vs_BLIND"])

    def test_saved_pass_flag_alone_is_insufficient(self):
        self.assertFalse(criterion({"pass": True}))
        data = self.comparison()
        data["n_seeds"] = 1
        self.assertFalse(criterion(data))
        data = self.comparison()
        data["exact_sign_p"] = float("nan")
        self.assertFalse(criterion(data))

    def test_best_core_uses_only_validation_not_test_peak(self):
        records = dict(rows=[dict(seed=s, architecture=arch, training_mode="BODY", best_validation_utility=.6 if arch == "GRU128" else .5,
                                 test_utility=.1 if arch == "GRU128" else 1.) for s in range(8) for arch in ("GRU128", "GRU64")])
        self.assertEqual(best_validation_core(records)["architecture"], "GRU128")

    def test_failed_probe_remains_fail_even_if_policy_files_exist(self):
        self.fixture()
        self.write("prediction_probe.json", dict(gate={"pass": False}))
        _, result = generate(self.root, output=self.out)
        self.assertFalse(result["gates"]["probe_validation_gate"])
        self.assertIn("validation gate が通っていない", (self.out / "K0_F_REPORT.md").read_text())

    def test_nonfinite_saved_json_is_rejected_and_reported(self):
        self.fixture()
        (self.root / "live_results.json").write_text('{"value":NaN}')
        stats, result = generate(self.root, output=self.out)
        self.assertEqual(result["research_status"], "FAIL")
        self.assertFalse(result["gates"]["evidence_readable"])
        self.assertIn("live/live_results.json", stats["read_errors"][0])

    def test_exploratory_scope_cannot_pass_even_when_all_metrics_pass(self):
        self.fixture()
        path = self.root / "training_config.json"
        config = json.loads(path.read_text())
        config["experiment_scope"] = "exploratory_after_failed_probe"
        self.write(path.name, config)
        stats, result = generate(self.root, output=self.out)
        self.assertEqual(result["research_status"], "FAIL")
        self.assertFalse(result["gates"]["confirmatory_scope"])
        self.assertEqual(stats["experiment_scope"], "exploratory_after_failed_probe")
        report = (self.out / "K0_F_REPORT.md").read_text()
        self.assertIn("後続比較が良好でも研究全体FAILを固定", report)
        self.assertIn("別学習BLINDを含む全結果", report)
        self.assertIn("BODY_vs_independently_trained_BLIND", report)


if __name__ == "__main__":
    unittest.main()
