import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest import mock

import numpy as np

from experiments.k0_f_interoception.probe import BODY_SCALE_FLOOR, PROBE_VERSION, features, fit_ridge, predict, run_probe
from experiments.k0_f_interoception import probe
from experiments.k0_f_interoception.tests.test_policy import fixture_rows


class ProbeTests(unittest.TestCase):
    def test_train_fit_no_heldout_scaling(self):
        x = np.array([[0., 2.], [1., 3.], [2., 4.]])
        model = fit_ridge(x, np.array([0., 1., 2.]))
        center = model["x_center"].copy()
        predict(model, np.array([[1e8, -1e8]]))
        np.testing.assert_array_equal(model["x_center"], center)

    def test_body_scaling_floor_preserves_task_scaling(self):
        x = np.zeros((4, 7))
        x[:, 0] = [0, .0001, .0002, .0003]
        x[:, 4] = [0, .00001, .00002, .00003]
        x[:, 5] = [0, .3, .6, .9]
        x[:, 6] = 1
        model = fit_ridge(x, np.arange(4))
        self.assertAlmostEqual(model["x_scale"][0], x[:, 0].std())
        self.assertEqual(model["x_scale"][1], 1.)
        self.assertEqual(model["x_scale"][4], BODY_SCALE_FLOOR)
        self.assertAlmostEqual(model["x_scale"][5], x[:, 5].std())
        self.assertEqual(model["x_scale"][6], BODY_SCALE_FLOOR)
        self.assertEqual(model["probe_version"], PROBE_VERSION)
        self.assertEqual(model["ridge_lambda"], 10.)

    def test_gate_and_no_test_examination(self):
        rows = fixture_rows()
        result, _, predictions = run_probe(rows)
        self.assertTrue(result["gate"]["pass"])
        self.assertEqual(result["schema_version"], "k0-f-probe-v2")
        self.assertEqual(result["body_scale_floor"], .05)
        self.assertEqual(result["gate"]["threshold_relative_mae_improvement"], .10)
        self.assertFalse(result["test_evaluated"])
        self.assertTrue(all(r["split"] == "validation" for r in predictions))
        mutated = copy.deepcopy(rows)
        for row in mutated:
            if row["split"] == "test":
                row["probe_targets"] = {"future_rtx3060_util": 1e9}
        second, _, _ = run_probe(mutated)
        self.assertEqual(result, second)

    def test_no_body_information_fails_gate(self):
        rows = fixture_rows()
        for row in rows:
            row["body_mask_sequence"] = np.zeros_like(row["body_mask_sequence"]).tolist()
        result, _, _ = run_probe(rows)
        self.assertFalse(result["gate"]["pass"])

    def test_features_exclude_target_block_and_absolute_time(self):
        rows = fixture_rows()[:4]
        changed = copy.deepcopy(rows)
        for row in changed:
            row.update(timestamp=1234567, workload_label="secret", costs_seconds=[10, 20, 30])
            row["probe_targets"] = {"future_rtx3060_util": 999}
        np.testing.assert_array_equal(features(rows, "BODY"), features(changed, "BODY"))

    def test_heldout_probe_never_called_without_prevalidated_gate(self):
        for gate_kind in ("missing", "failed"):
            with self.subTest(gate_kind=gate_kind), tempfile.TemporaryDirectory() as temporary:
                output = Path(temporary)
                if gate_kind == "failed":
                    (output / "prediction_probe.json").write_text(json.dumps({"gate": {"pass": False}, "test_evaluated": False}))
                argv = ["probe", "--dataset", str(output / "unreadable-dataset.jsonl"), "--output", str(output), "--include-test"]
                with mock.patch("sys.argv", argv), mock.patch.object(probe, "run_probe") as run, mock.patch.object(probe, "load_dataset") as load:
                    with self.assertRaises(RuntimeError):
                        probe.main()
                    run.assert_not_called()
                    load.assert_not_called()


if __name__ == "__main__":
    unittest.main()
