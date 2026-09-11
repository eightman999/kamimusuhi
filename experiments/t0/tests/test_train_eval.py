import json
import tempfile
import unittest
from pathlib import Path

import torch

from experiments.t0.env.interval_env import IntervalEnv
from experiments.t0.evaluate import (hidden_noise, hidden_reset, obs_blank,
                                     eval_delay, eval_with_intervention,
                                     consolidate)
from experiments.t0.models import make_model
from experiments.t0.train import (advantages, collect, episode_metrics, train)


class TestCollect(unittest.TestCase):
    def test_shapes_and_mask(self):
        model = make_model("gru64")
        env = IntervalEnv(8, "cpu", 1, {"delays": [8], "horizon": 20})
        rollout, mask, infos, states = collect(model, env, record_states=True)
        self.assertEqual(rollout["obs"].shape[1:], (8, 8))
        self.assertEqual(mask.shape[0], rollout["obs"].shape[0])
        self.assertEqual(states.shape[:2], mask.shape)
        # episodes end at ACT-or-horizon; mask is prefix-true per env
        for b in range(8):
            col = mask[:, b]
            if bool((~col).any()):
                first_dead = int((~col).float().argmax())
                self.assertFalse(bool(col[first_dead:].any()))

    def test_greedy_oracle_policy_succeeds(self):
        env = IntervalEnv(16, "cpu", 2, {"delays": [16], "horizon": 40})

        class Teacher:
            num_actions = 3
            def initial_state(self, b, d):
                return torch.empty(b, 0, device=d)
            def __call__(self, o, s):
                return torch.zeros(o.shape[0], 3), torch.zeros(o.shape[0]), s
        rollout, mask, infos = collect(Teacher(), env, teacher_probability=1.)[:3]
        self.assertTrue(bool(infos[-1]["success"].all()))


class TestAdvantages(unittest.TestCase):
    def test_terminal_masking(self):
        rewards = torch.zeros(6, 2)
        values = torch.zeros(6, 2)
        mask = torch.ones(6, 2, dtype=torch.bool)
        rewards[4, 0] = 1.
        mask[5, 0] = False
        adv, ret = advantages(rewards, values, mask, gamma=1., lam=1.)
        self.assertAlmostEqual(float(adv[4, 0]), 1.)
        self.assertAlmostEqual(float(adv[0, 0]), 1.)
        self.assertAlmostEqual(float(adv[5, 0]), 0.)


class TestTrainSmoke(unittest.TestCase):
    def test_short_run_produces_artifacts(self):
        with tempfile.TemporaryDirectory() as d:
            config = {"task": "interval", "architecture": "gru64", "seed": 0,
                      "num_envs": 32, "minibatch_envs": 32,
                      "imitation_updates": 2, "ppo_updates": 1,
                      "ppo_epochs": 1, "validation_envs": 16, "threads": 1,
                      "env": {"delays": [8, 16], "horizon": 32,
                              "delay_distribution": "grid"}}
            train(config, d, "smoke")
            run = Path(d) / "runs" / "smoke"
            for name in ("initial.pt", "imitation_final.pt", "ppo_final.pt",
                         "metrics.jsonl", "status.json", "config.json"):
                self.assertTrue((run / name).exists(), name)
            status = json.loads((run / "status.json").read_text())
            self.assertEqual(status["status"], "complete")


class TestInterventions(unittest.TestCase):
    def setUp(self):
        torch.manual_seed(0)
        self.model = make_model("gru64")
        self.config = {"env": {"delays": [16], "horizon": 40}}

    def test_eval_delay(self):
        out = eval_delay(self.model, "cpu", 16, n=16, config=self.config)
        for key in ("success", "act_rate", "early_rate", "late_rate"):
            self.assertIn(key, out)

    def test_interventions_run(self):
        for fn in (hidden_reset(.5), hidden_noise(.5), obs_blank(0., 1.)):
            out = eval_with_intervention(self.model, "cpu", 16, 16,
                                         self.config, fn)
            self.assertIn("success", out)

    def test_hidden_reset_applies(self):
        """The intervention closure must actually zero the state tensor at the
        target step; behavioural effect is a research result, not a test."""
        class Waiter(torch.nn.Module):
            num_actions = 3
            def initial_state(self, b, d):
                return torch.ones(b, 8, device=d)
            def forward(self, o, s):
                logits = torch.zeros(o.shape[0], 3)
                logits[:, 0] = 10.
                return logits, torch.zeros(o.shape[0]), s + 1
        env = IntervalEnv(16, "cpu", 4, {"delays": [16], "horizon": 40})
        env.reset()
        seen = []

        def probe(t, state, observation):
            hit = t == torch.round(env.target_step.float() * .5).long()
            state, observation = hidden_reset(.5)(t, state, observation, env)
            if bool(hit.any()):
                seen.append(float(state[hit].abs().max()))
            return state, observation

        collect(Waiter(), env, greedy=True, intervention=probe)
        self.assertTrue(seen)
        self.assertTrue(all(v == 0. for v in seen))


class TestConsolidate(unittest.TestCase):
    def test_consolidate_structure(self):
        with tempfile.TemporaryDirectory() as d:
            eval_dir = Path(d) / "eval"
            eval_dir.mkdir(parents=True)
            row = {"architecture": "gru64", "seed": 0,
                   "seen_mean_success": .9, "interpolation_mean_success": .8,
                   "extrapolation_mean_success": .3,
                   "interventions": {"hidden_reset": {"success": .2},
                                     "baseline": {"success": .9}},
                   "probes": {"elapsed_r2": .95}}
            (eval_dir / "a.json").write_text(json.dumps(row))
            summary = consolidate(d)
            self.assertIn("gru64", summary)
            self.assertEqual(summary["gru64"]["seen"]["n"], 1)


if __name__ == "__main__":
    unittest.main()
