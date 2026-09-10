import copy
import tempfile
import unittest
from pathlib import Path

import numpy as np
import torch
from torch import nn

from experiments.k0_f_interoception.policy import (
    ACTION_NAMES, BodyPolicy, SENSOR_OOD, TEMPORAL_OOD, counterfactual,
    encode_inputs, infer, load_model, score_actions, train_one, utilities, validate_rows,
)
from experiments.k0_f_interoception.statistics import describe, exact_sign_test, paired_comparison


def fixture_rows():
    rows = []
    for split in ("train", "validation", "test"):
        for block in range(4):
            for i in range(4):
                busy = block % 2
                values = np.zeros((1 + i, 20), dtype=float)
                values[:, 5], values[:, 9] = busy, 1 - busy
                values[:, 18] = .01
                values[:, 19] = 1
                stale = values.copy()
                stale[:, 5], stale[:, 9] = 1 - busy, busy
                rows.append({"schema_version": "k0-f-policy-v1", "split": split,
                    "block_id": f"{split}-block{block}", "episode_id": f"{split}-{block}-{i}",
                    "task_features": [.5, .5, .2, 0], "body_sequence": values.tolist(),
                    "body_mask_sequence": np.ones_like(values).tolist(),
                    "stale_body_sequence": stale.tolist(), "stale_body_mask_sequence": np.ones_like(values).tolist(),
                    "costs_seconds": [2, 1.8 if busy else .1, .1 if busy else 1.8],
                    "failures": [False, False, False], "deadline_seconds": 1.,
                    "probe_targets": {"future_rtx3060_util": float(busy),
                        "future_p100_util": float(1 - busy), "next_job_latency_seconds": .1 + busy}})
    return rows


class GreedyBodyModel(nn.Module):
    """Deterministic fixture decision, not a measured research model."""
    def __init__(self):
        super().__init__()
        self.dummy = nn.Parameter(torch.zeros(1))
        self.hidden_size = 1

    def forward(self, sequences, hidden=None):
        logits = torch.stack([torch.stack([self.dummy[0] - 10, -x[-1, 4 + 5], -x[-1, 4 + 9]]) for x in sequences])
        return logits, torch.ones((1, len(sequences), 1))


class PolicyTests(unittest.TestCase):
    def test_no_target_metadata_leak(self):
        row = fixture_rows()[0]
        changed = copy.deepcopy(row)
        changed.update(costs_seconds=[900., .0001, 5.], block_id="secret-label", timestamp=10**10,
                       workload_label="teacher-P100", probe_targets={"future_rtx3060_util": .9})
        self.assertTrue(torch.equal(encode_inputs([row], "BODY")[0], encode_inputs([changed], "BODY")[0]))

    def test_missing_never_normal_value(self):
        row = fixture_rows()[0]
        missing = copy.deepcopy(row)
        missing["body_mask_sequence"][-1][5] = 0
        encoded = encode_inputs([missing], "BODY")[0]
        self.assertEqual(encoded[-1, 4 + 20 + 5], 0)
        self.assertFalse(torch.equal(encoded, encode_inputs([row], "BODY")[0]))
        blind = encode_inputs([row], "BLIND")[0]
        self.assertEqual(blind[:, 4:].abs().sum(), 0)
        self.assertTrue(torch.equal(blind[:, :4], encoded[:, :4]))

    def test_split_and_id_guards(self):
        rows = fixture_rows()
        self.assertTrue(validate_rows(rows)["blocks_disjoint"])
        changed = copy.deepcopy(rows)
        changed[17]["block_id"] = changed[0]["block_id"]
        with self.assertRaisesRegex(ValueError, "crosses"):
            validate_rows(changed)
        changed = copy.deepcopy(rows)
        changed[-1]["episode_id"] = changed[0]["episode_id"]
        with self.assertRaisesRegex(ValueError, "duplicate"):
            validate_rows(changed)

    def test_shuffle_preserves_marginals_tasks_and_is_cross_block(self):
        rows = [r for r in fixture_rows() if r["split"] == "test"]
        for i, row in enumerate(rows):
            row["body_sequence"][-1][0] = i / len(rows)
        ordinary = encode_inputs(rows, "BODY")
        shuffled = encode_inputs(rows, "SHUFFLED", 8)
        self.assertEqual(sorted(float(x[-1, 4]) for x in ordinary), sorted(float(x[-1, 4]) for x in shuffled))
        for row, x in zip(rows, shuffled):
            donor = round(float(x[-1, 4]) * len(rows))
            self.assertNotEqual(row["block_id"], rows[donor]["block_id"])
            np.testing.assert_array_equal(x[-1, :4], np.asarray(row["task_features"], dtype=np.float32))

    def test_ood_finite_masked_inputs(self):
        rows = fixture_rows()[:4]
        model = BodyPolicy(8)
        for mode in SENSOR_OOD + TEMPORAL_OOD:
            with self.subTest(mode=mode):
                inputs = encode_inputs(rows, mode, 3)
                self.assertTrue(all(torch.isfinite(x).all() for x in inputs))
                actions, norms = infer(model, inputs)
                self.assertEqual(len(actions), len(rows))
                self.assertTrue(np.isfinite(norms).all())

    def test_reward_failure_and_deadline(self):
        row = fixture_rows()[0]
        self.assertEqual(utilities(row).tolist(), [-1., .9, -.8])
        row["failures"][1] = True
        self.assertEqual(utilities(row)[1], -1)
        self.assertEqual(score_actions([row], [1])["failure_rate"], 1)

    def test_counterfactual_requires_outcome_gain(self):
        rows = [r for r in fixture_rows() if r["split"] == "test"]
        result, pairs = counterfactual(GreedyBodyModel(), rows, 0)
        self.assertGreater(result["action_change_rate"], 0)
        self.assertGreater(result["utility_gain"], 0)
        self.assertLess(result["matched_latency_seconds"], result["frozen_latency_seconds"])
        self.assertTrue(all(p["matched_action"] in ACTION_NAMES for p in pairs))

    def test_training_checkpoint_and_split_smoke(self):
        torch.set_num_threads(1)
        rows = fixture_rows()
        train = [r for r in rows if r["split"] == "train"]
        validation = [r for r in rows if r["split"] == "validation"]
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "run"
            result = train_one(train, validation, path, epochs=3, hidden_size=8, batch_size=8)
            model, meta = load_model(path / "best.pt")
            self.assertEqual(meta["checkpoint_stage"], "best")
            self.assertEqual(meta["best_validation_utility"], result["best_validation_utility"])
            self.assertTrue(all(torch.isfinite(v).all() for v in model.parameters()))
            with self.assertRaises(FileExistsError):
                train_one(train, validation, path, epochs=1)
            with self.assertRaises(ValueError):
                train_one([rows[-1]], validation, Path(temporary) / "leak", epochs=1)

    def test_seed_statistics_no_episode_n(self):
        self.assertEqual(exact_sign_test([1.] * 8), .0078125)
        self.assertEqual(exact_sign_test([0.] * 8), 1.)
        result = paired_comparison([1.] * 8, [.5] * 8)
        self.assertTrue(result["pass"])
        self.assertEqual(result["n_seeds"], 8)
        self.assertEqual(result["ci95"], [.5, .5])
        self.assertAlmostEqual(describe([1, 2, 3])["sd"], 1)


if __name__ == "__main__":
    unittest.main()
