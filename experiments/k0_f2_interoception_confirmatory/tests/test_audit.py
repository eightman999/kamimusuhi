"""Synthetic-only independent-audit regression tests; never use K0-F records."""
import copy
import math
import unittest

import numpy as np
import torch

from experiments.k0_f2_interoception_confirmatory.audit import (
    ACTIONS, AuditError, action_utilities, audit_counterfactual, audit_input_traces,
    audit_live, audit_probe, audit_rows, audit_sessions, audit_shuffle,
    expected_policy_sequence, independent_describe, independent_gru_inference,
    independent_pair, independent_probe_features, independent_sign_test, unique_seed_values,
)


def row(session="A", block=0, task=0, length=1):
    tasks = ((128, 8, .003), (512, 16, .008), (1024, 16, .025), (2048, 8, .07))
    n, repetitions, deadline = tasks[task]
    value = [[.1] * 20 for _ in range(length)]
    return {"schema_version": "k0-f2-policy-v1", "session_id": session,
            "block_id": f"{session}-{block}", "episode_id": f"{session}-{block}-{task}-{length}",
            "split": {"A": "train", "B": "train", "C": "validation", "D": "test"}[session],
            "timestamp": 100., "task_id": task,
            "task_features": [n / 2048, repetitions / 16, deadline / .1, 3*n*n*4/(64*1024**2)],
            "protocol_lock_sha256": "lock", "body_sequence": value,
            "body_mask_sequence": [[1] * 20 for _ in range(length)],
            "stale_body_sequence": copy.deepcopy(value), "stale_body_mask_sequence": [[1] * 20 for _ in range(length)],
            "costs_seconds": [.002, .001, .0015, .021], "failures": [False] * 4, "deadline_seconds": deadline,
            "provenance": {"frame_ids": [f"{session}-now-{i}" for i in range(length)],
                "stale_frame_ids": [f"{session}-old-{i}" for i in range(length)],
                "telemetry_kind": "real", "normalization_identity": "norm"}}


class AuditTests(unittest.TestCase):
    def test_independent_seed_gate_effect_and_ties(self):
        result = independent_pair({i:.7 for i in range(12)}, {i:.5 for i in range(12)})
        self.assertTrue(result["pass"])
        self.assertEqual(result["exact_sign_p"], 2/4096)
        self.assertFalse(independent_pair({i:.505 for i in range(12)}, {i:.5 for i in range(12)})["pass"])
        self.assertEqual(independent_sign_test([0]*12)["exact_sign_p"], 1)
        self.assertEqual(independent_sign_test([1]*9+[-1]*3)["exact_sign_p"], .14599609375)
        self.assertAlmostEqual(independent_describe([1,2,3])["sd"], 1)
        with self.assertRaises(AuditError):
            unique_seed_values([{"seed":0,"utility":.1},{"seed":0,"utility":.2}])
        with self.assertRaises(AuditError):
            independent_pair({0:1}, {1:0})

    def test_fresh_sessions_buffer_and_hash_reuse(self):
        sessions = [{"session_id":s,"split":split,"start_timestamp":100+i*100,"end_timestamp":150+i*100,
                     "protocol_lock_sha256":"x","raw_mac_sha256":str(i+1)*64,"raw_master_sha256":str(i+5)*64}
                    for i,(s,split) in enumerate((('A','train'),('B','train'),('C','validation'),('D','test')))]
        self.assertTrue(audit_sessions(sessions,lock_sha256="x",locked_at=90)["pass"])
        changed=copy.deepcopy(sessions);changed[1]["raw_mac_sha256"]=changed[0]["raw_mac_sha256"]
        with self.assertRaises(AuditError):audit_sessions(changed,lock_sha256="x",locked_at=90)
        changed=copy.deepcopy(sessions);changed[1]["start_timestamp"]=151
        with self.assertRaises(AuditError):audit_sessions(changed,lock_sha256="x",locked_at=90)

    def test_real_boundary_rejects_smoke_mask_future_and_target_features(self):
        original = row()
        kwargs=dict(lock_sha256="lock",normalization_identity="norm",expected_decisions_per_block=None,require_measurements=False)
        self.assertTrue(audit_rows([original],**kwargs)["pass"])
        for mutate in (
            lambda r:r["provenance"].update(telemetry_kind="synthetic"),
            lambda r:r["body_mask_sequence"][0].__setitem__(0,.5),
            lambda r:r["task_features"].__setitem__(0,1000000000),
            lambda r:r.update(split="test"),
        ):
            changed=copy.deepcopy(original);mutate(changed)
            with self.assertRaises(AuditError):audit_rows([changed],**kwargs)
        changed=copy.deepcopy(original);changed["body_mask_sequence"][0][0]=0
        with self.assertRaises(AuditError):audit_rows([changed],**kwargs)
        frame={"schema_version":"k0f2.frame.v1","source_kind":"real","normalization_identity":"norm","values":[.1]*20,"mask":[1]*20,"timestamp":101}
        with self.assertRaisesRegex(AuditError,"timestamp"):
            audit_rows([original],frame_index={"A-now-0":frame},**kwargs)

    def test_shuffle_preserves_task_session_length_and_bijection(self):
        rows=[row(block=i) for i in range(3)]
        mapping={rows[i]["episode_id"]:rows[(i+1)%3]["episode_id"] for i in range(3)}
        self.assertTrue(audit_shuffle(rows,mapping)["pass"])
        bad=dict(mapping);bad[rows[0]["episode_id"]]=rows[0]["episode_id"]
        with self.assertRaises(AuditError):audit_shuffle(rows,bad)
        changed=copy.deepcopy(rows);changed[1]["task_features"][0]=1
        with self.assertRaises(AuditError):audit_shuffle(changed,mapping)

    def test_inputs_independent_of_outcomes_metadata(self):
        original=row(length=3);changed=copy.deepcopy(original)
        changed.update(costs_seconds=[10,20,30,40],timestamp=99999,teacher_action=2,workload_label="not-a-feature")
        np.testing.assert_array_equal(independent_probe_features(original),independent_probe_features(changed))
        self.assertEqual(len(independent_probe_features(original)),126)
        trace={"episode_id":original["episode_id"],"mode":"BODY","model_input_sequence":expected_policy_sequence(original)}
        self.assertTrue(audit_input_traces([original],[trace])["pass"])
        trace["model_input_sequence"][0][0]=original["costs_seconds"][0]
        with self.assertRaises(AuditError):audit_input_traces([original],[trace])

    def test_independent_gru_matches_torch_without_training(self):
        from experiments.k0_f2_interoception_confirmatory.policy import BodyPolicy
        torch.manual_seed(901);torch.set_num_threads(1)
        model=BodyPolicy(8).eval();sequence=expected_policy_sequence(row(length=4))
        with torch.no_grad():
            logits,hidden=model([torch.tensor(sequence,dtype=torch.float32)])
        action,actual,state=independent_gru_inference(model.state_dict(),sequence)
        np.testing.assert_allclose(actual,logits[0].numpy(),atol=1e-6,rtol=1e-6)
        np.testing.assert_allclose(state,hidden[-1,0].numpy(),atol=1e-6,rtol=1e-6)
        self.assertEqual(action,int(logits.argmax(-1)))

    def test_counterfactual_all_pairs_and_destination_cost(self):
        rows=[row(session="D",block=i) for i in range(3)]; pairs=[]
        for seed in range(12):
            for a in rows:
                for b in rows:
                    if a["block_id"]==b["block_id"]:continue
                    u=action_utilities(b)
                    pairs.append({"seed":seed,"source_episode_id":a["episode_id"],"destination_episode_id":b["episode_id"],"matched_action":1,"frozen_action":0,"matched_utility":u[1],"frozen_utility":u[0]})
        self.assertTrue(audit_counterfactual(rows,pairs)["pass"])
        with self.assertRaises(AuditError):audit_counterfactual(rows,pairs[:-1])
        bad=copy.deepcopy(pairs);bad[0]["matched_utility"]=100
        with self.assertRaises(AuditError):audit_counterfactual(rows,bad)

    def test_action_utility_wait_and_failure(self):
        sample=row();u=action_utilities(sample)
        self.assertEqual(u[3],-1)
        sample["failures"][1]=True
        self.assertEqual(action_utilities(sample)[1],-1)


if __name__ == "__main__":
    unittest.main()
