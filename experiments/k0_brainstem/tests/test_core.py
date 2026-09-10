import unittest
import torch
from experiments.k0_brainstem.env.vector_env import VectorEnv
from experiments.k0_brainstem.env.signals import INVOKE_LANGUAGE
from experiments.k0_brainstem.models import make_model


class EnvironmentTests(unittest.TestCase):
    def test_all_scenarios_shapes_reward_and_seed(self):
        a, b = VectorEnv(1024, 64, seed=9), VectorEnv(1024, 64, seed=9)
        obs = a.reset()
        self.assertTrue(torch.equal(obs, b.reset()))
        self.assertEqual(set(a.scenario.tolist()), set(range(8)))
        self.assertEqual(obs.shape, (1024, 16))
        self.assertTrue(((a._data["observations"] >= 0) & (a._data["observations"] <= 1)).all())
        for step in range(64):
            target = a.oracle_actions()
            obs, reward, done, info = a.step(target)
            self.assertTrue(info["success"].all())
            self.assertTrue((reward >= .85 - 1e-6).all())
            self.assertEqual(bool(done.all()), step == 63)
        with self.assertRaises(RuntimeError):
            a.step(target)

    def test_delayed_cue_no_decision_label_leakage(self):
        env = VectorEnv(2048, 128, seed=5)
        env.reset()
        data = env._data
        mask = data["scenario"] == 3
        self.assertTrue((data["delay"][mask] >= 24).all())
        # Generate counterfactual episodes with every cue identity flipped but
        # the exact same RNG draws. The generator must not leak it after t=0.
        selected = torch.where(mask)[0]
        counterpart = VectorEnv(2048, 128, seed=5)
        counterpart.reset(cue_override=1 - data["cue"])
        self.assertTrue(torch.equal(data["observations"][1:], counterpart._data["observations"][1:]))
        times = data["delay"][selected]
        self.assertTrue((data["targets"][times, selected] != counterpart._data["targets"][times, selected]).all())
        self.assertTrue(torch.equal(data["delay"], counterpart._data["delay"]))
        # Explicit decision marker has the same exact values under both labels.
        decision = data["observations"][data["delay"][selected], selected]
        for label in [0, 1]:
            row = decision[data["cue"][selected] == label]
            self.assertTrue((row[:, 5] == .4).all())
            self.assertTrue((row[:, 14] == .9).all())
        # A clock-based stateless predictor has no cue-dependent signal.
        for label in [0, 1]:
            self.assertGreater(int((data["cue"][selected] == label).sum()), 80)

    def test_language_requires_history(self):
        env = VectorEnv(1024, 64, seed=11)
        env.reset()
        obs, target = env._data["observations"], env._data["targets"]
        calls = target == INVOKE_LANGUAGE
        false_speech = (env.scenario == 7)[None, :] & (obs[..., 8] == .9)
        self.assertGreater(int(calls.sum()), 0)
        for feature in [2, 4, 7, 8, 14]:
            self.assertEqual(float(obs[..., feature][calls][0]), float(obs[..., feature][false_speech][0]))
        self.assertFalse((calls & (env.scenario == 7)[None, :]).any())
        self.assertTrue((obs[:-1, :, 8][calls[1:]] == .9).all())

    def test_heldout_delay(self):
        env = VectorEnv(512, 128, seed=5, ood="longer_delay")
        env.reset()
        self.assertTrue((env._data["delay"] >= 48).all())

    def test_ood_aliases_and_invalid(self):
        for alias, canonical in [("long_delay", "longer_delay"), ("sensor_fault", "failure")]:
            a = VectorEnv(64, 128, seed=3, ood=alias)
            b = VectorEnv(64, 128, seed=3, ood=canonical)
            self.assertTrue(torch.equal(a.reset(), b.reset()))
            self.assertTrue(torch.equal(a._data["delay"], b._data["delay"]))
        with self.assertRaisesRegex(ValueError, "Unknown OOD"):
            VectorEnv(4, 64, ood="typo").reset()

    def test_habituation_three_stage_masks(self):
        env = VectorEnv(256, 64, seed=2)
        env.reset()
        data = env._data
        repeated = data["scenario"] == 1
        self.assertTrue((data["first_stimulus_mask"][:, repeated].sum(0) == 1).all())
        self.assertTrue((data["habituation_mask"][:, repeated].sum(0) == 5).all())
        self.assertTrue((data["renewed_stimulus_mask"][:, repeated].sum(0) == 1).all())
        self.assertTrue((data["targets"][data["first_stimulus_mask"]] == 2).all())
        self.assertTrue((data["targets"][data["habituation_mask"]] == 0).all())
        self.assertTrue((data["targets"][data["renewed_stimulus_mask"]] == 2).all())


class ModelTests(unittest.TestCase):
    def test_gru_sequence_equivalence_and_temporal_gradient(self):
        model = make_model("gru64")
        obs = torch.rand(12, 4, 16, requires_grad=True)
        initial = model.initial_state(4, "cpu")
        logits, values, final = model.forward_sequence(obs, initial)
        state = initial
        streamed = []
        for row in obs:
            out, value, state = model(row, state)
            streamed.append(out)
        self.assertTrue(torch.allclose(logits, torch.stack(streamed), atol=1e-6))
        logits[-1].sum().backward()
        self.assertGreater(float(obs.grad[0].abs().sum()), 0)
        self.assertEqual(final.shape, (4, 64))

    def test_all_architectures_train_and_sequence_contract(self):
        for architecture in ["mlp", "gru64", "gru128", "sparse_rnn", "softlogic_rnn", "fly_modular"]:
            with self.subTest(architecture=architecture):
                torch.manual_seed(7)
                model = make_model(architecture)
                observations = torch.rand(12, 3, 16, requires_grad=True)
                initial = model.initial_state(3, "cpu")
                logits, values, final = model.forward_sequence(observations, initial)
                state, streamed = initial, []
                for row in observations:
                    output, _, state = model(row, state)
                    streamed.append(output)
                self.assertTrue(torch.allclose(logits, torch.stack(streamed), atol=1e-6))
                self.assertEqual(logits.shape, (12, 3, 6))
                self.assertEqual(values.shape, (12, 3))
                logits[-1].sum().backward()
                earlier_gradient = float(observations.grad[0].abs().sum())
                if architecture == "mlp":
                    self.assertEqual(earlier_gradient, 0.)
                else:
                    self.assertGreater(earlier_gradient, 0.)
                optimizer = torch.optim.Adam(model.parameters(), lr=.001)
                optimizer.step()
                self.assertTrue(torch.isfinite(logits).all())
                self.assertEqual(model.stats()["params"], sum(p.numel() for p in model.parameters()))
                self.assertGreater(model.stats()["active_connections"], 0)

    def test_sparse_density_and_forbidden_gradient(self):
        for density in [.1, .25]:
            model = make_model("sparse_rnn", density=density)
            mask = model.recurrent.mask
            self.assertEqual(int(mask.sum()), round(mask.numel() * density))
            _, _, state = model.forward_sequence(torch.rand(5, 2, 16), model.initial_state(2, "cpu"))
            state.sum().backward()
            self.assertTrue((model.recurrent.weight.grad[mask == 0] == 0).all())
            expected = int(mask.sum()) + 128 * 16 + 128 * 7
            self.assertEqual(model.stats()["active_connections"], expected)

    def test_logic_discrete_truth_tables(self):
        model = make_model("softlogic_rnn")
        with torch.no_grad():
            model.source_a.fill_(0)
            model.source_b.fill_(1)
            model.retention_logit.fill_(-40)
            model.gate_logits.fill_(-100)
            for unit in range(model.hidden_size):
                model.gate_logits[unit, unit % 4] = 100
        model.eval().set_discretized(True)
        observation = torch.zeros(4, 16)
        observation[:, :2] = torch.tensor([[0, 0], [0, 1], [1, 0], [1, 1]])
        _, _, state = model(observation, model.initial_state(4, "cpu"))
        expected = torch.tensor([[0, 0, 0, 1], [0, 1, 1, 1], [0, 1, 1, 1], [1, 1, 0, 0]], dtype=torch.float)
        self.assertTrue(torch.allclose(state[:, :4], expected, atol=1e-6))
        model.set_discretized(False)
        self.assertFalse(model.discretized)

    def test_fly_module_connectivity(self):
        model = make_model("fly_modular")
        mask = model.recurrent.mask
        allowed = model.allowed_connections
        self.assertTrue((mask[allowed == 0] == 0).all())
        self.assertEqual(int(mask.sum()), round(int(allowed.sum()) * .1))
        logits, _, state = model.forward_sequence(torch.rand(8, 2, 16), model.initial_state(2, "cpu"))
        logits.sum().backward()
        self.assertTrue((model.recurrent.weight.grad[mask == 0] == 0).all())
        metrics = model.state_metrics(state)
        for module in model.module_names:
            self.assertIn("module_" + module, metrics)


if __name__ == "__main__":
    unittest.main()
