import unittest

import torch

from experiments.t0.models import ARCHITECTURES, make_model


class TestModels(unittest.TestCase):
    def test_shapes(self):
        for arch in ARCHITECTURES:
            model = make_model(arch)
            state = model.initial_state(8, "cpu")
            self.assertEqual(state.shape[0], 8)
            obs = torch.rand(8, model.obs_dim)
            logits, value, state = model(obs, state)
            self.assertEqual(logits.shape, (8, model.num_actions))
            self.assertEqual(value.shape, (8,))

    def test_sequence_matches_loop(self):
        for arch in ("gru64", "lstm64", "ssm", "leaky"):
            torch.manual_seed(0)
            model = make_model(arch)
            obs_seq = torch.rand(12, 8, model.obs_dim)
            state = model.initial_state(8, "cpu")
            seq_logits, seq_values, seq_state = model.forward_sequence(obs_seq, state)
            loop_state = model.initial_state(8, "cpu")
            logits, values = [], []
            for t in range(12):
                lo, va, loop_state = model(obs_seq[t], loop_state)
                logits.append(lo)
                values.append(va)
            self.assertTrue(torch.allclose(seq_logits, torch.stack(logits), atol=1e-5),
                            f"{arch} sequence/loop mismatch")
            self.assertTrue(torch.allclose(seq_state, loop_state, atol=1e-5),
                            f"{arch} final state mismatch")

    def test_mlp_has_no_memory(self):
        model = make_model("mlp")
        state = model.initial_state(4, "cpu")
        self.assertEqual(state.numel(), 0)

    def test_state_is_single_tensor(self):
        for arch in ARCHITECTURES:
            model = make_model(arch)
            state = model.initial_state(4, "cpu")
            self.assertTrue(torch.is_tensor(state),
                            f"{arch} state must be one tensor for interventions")


if __name__ == "__main__":
    unittest.main()
