import copy
import unittest

import numpy as np

from experiments.k0_f_interoception.probe import features, fit_ridge, predict, run_probe
from experiments.k0_f_interoception.tests.test_policy import fixture_rows


class ProbeTests(unittest.TestCase):
    def test_train_fit_no_heldout_scaling(self):
        x = np.array([[0., 2.], [1., 3.], [2., 4.]])
        model = fit_ridge(x, np.array([0., 1., 2.]))
        center = model["x_center"].copy()
        predict(model, np.array([[1e8, -1e8]]))
        np.testing.assert_array_equal(model["x_center"], center)

    def test_gate_and_no_test_examination(self):
        rows = fixture_rows()
        result, _, predictions = run_probe(rows)
        self.assertTrue(result["gate"]["pass"])
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


if __name__ == "__main__":
    unittest.main()
