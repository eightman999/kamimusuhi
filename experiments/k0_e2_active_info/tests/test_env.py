import unittest
import torch
from experiments.k0_e2_active_info.env import ActiveInfoEnv, IGNORE, WAIT, ORIENT, OBSERVE, RECALL, INVOKE_LANGUAGE


class ActiveInfoTests(unittest.TestCase):
    def environment(self, scenario="language", **config):
        env = ActiveInfoEnv(64, "cpu", 17, {"scenario": scenario, "episode_length": 24, **config})
        env.reset()
        return env

    def act(self, env, action):
        return env.step(torch.full((env.num_envs,), action, dtype=torch.long))

    def test_closed_loop_orient_observe_wait(self):
        for scenario, action in [("orient", ORIENT), ("observe", OBSERVE)]:
            base = self.environment(scenario)
            acquired = base.clone()
            no, _, _, _ = self.act(base, IGNORE)
            yes, _, _, info = self.act(acquired, action)
            self.assertFalse(torch.equal(no, yes))
            self.assertTrue((yes[:, 10] > 0).all())
            self.assertTrue((no[:, 10] == 0).all())
            self.assertTrue(info["acquisition"].all())
        wait = self.environment()
        before = wait.resource.clone()
        self.act(wait, WAIT)
        self.assertTrue((wait.resource > before).all())

    def test_orient_expires_after_three_observations(self):
        env = self.environment("orient", orient_duration=3)
        self.act(env, ORIENT)
        self.assertTrue((env.observation[:, 10] > 0).all())
        for _ in range(2):
            self.act(env, IGNORE)
            self.assertTrue((env.observation[:, 10] > 0).all())
        self.act(env, IGNORE)
        self.assertTrue((env.observation[:, 10] == 0).all())

    def test_ambiguity_all_nonlanguage_actions_bitidentical(self):
        a = self.environment()
        b = self.environment()
        b.reset()  # reset state is replaced below, preserving original exogenous RNG
        b = ActiveInfoEnv(64, "cpu", 17, a.config)
        b.reset(latent_override=1 - a.latent)
        self.assertTrue(torch.equal(a.observation, b.observation))
        for t in range(a.episode_length):
            action = [IGNORE, WAIT, ORIENT, OBSERVE, RECALL][t % 5]
            oa, _, _, ia = self.act(a, action)
            ob, _, _, ib = self.act(b, action)
            self.assertTrue(torch.equal(oa, ob), f"history differs at {t}")
            self.assertTrue(torch.equal(ia["oracle_action"], ib["oracle_action"]))
        self.assertTrue((ia["target"] != ib["target"]).all())

    def test_call_return_different_then_downstream_answer(self):
        a = self.environment(language_latency=0)
        b = ActiveInfoEnv(64, "cpu", 17, a.config)
        b.reset(latent_override=1 - a.latent)
        self.act(a, INVOKE_LANGUAGE)
        self.act(b, INVOKE_LANGUAGE)
        self.assertTrue((a.observation[:, 9] != b.observation[:, 9]).all())
        self.assertTrue((a.observation[:, 10] == 1).all())
        while a.t < a.episode_length - 1:
            self.act(a, IGNORE)
            self.act(b, IGNORE)
        self.assertTrue((a.oracle_actions() != b.oracle_actions()).all())
        _, _, _, ia = a.step(a.oracle_actions())
        _, _, _, ib = b.step(b.oracle_actions())
        self.assertTrue(ia["success"].all())
        self.assertTrue(ib["success"].all())

    def test_latency_and_cost_exactly_once(self):
        for latency in [0, 1, 2, 4, 8, 16]:
            env = self.environment(language_latency=latency, language_cost=.2)
            charges, deliveries = [], []
            for t in range(latency + 2):
                _, reward, _, info = self.act(env, INVOKE_LANGUAGE)
                charges.append(info["language_cost"])
                deliveries.append(info["language_delivered"])
                self.assertFalse(info["success"].any())
                self.assertEqual(bool(info["language_delivered"].all()), t == latency)
            self.assertTrue(torch.allclose(torch.stack(charges).sum(0), torch.full((64,), .2)))
            self.assertTrue((torch.stack(deliveries).sum(0) == 1).all())

    def test_response_interventions(self):
        base = self.environment(language_latency=0)
        results = {}
        for mode in ["correct", "shuffled", "random", "inverted", "delayed", "missing", "plausible_wrong", "contradictory", "uncertain"]:
            env = base.clone()
            env.response_mode = mode
            self.act(env, INVOKE_LANGUAGE)
            results[mode] = env
        self.assertTrue(torch.equal(results["correct"].language_fact, base.latent))
        self.assertTrue(torch.equal(results["inverted"].language_fact, 1 - base.latent))
        self.assertTrue(torch.equal(results["plausible_wrong"].language_fact, 1 - base.latent))
        self.assertTrue((results["missing"].observation[:, 10] == 0).all())
        self.assertTrue((results["contradictory"].observation[:, 10] == 0).all())
        self.assertTrue((results["uncertain"].observation[:, 10] == .5).all())
        self.assertFalse(results["delayed"].delivered.any())
        self.assertFalse(torch.equal(results["shuffled"].language_fact, base.latent))
        self.assertFalse(torch.equal(results["random"].language_fact, base.latent))

    def test_rng_snapshot_restore_and_fork(self):
        env = self.environment()
        self.act(env, INVOKE_LANGUAGE)
        saved = env.snapshot()
        fork = env.clone()
        for action in [WAIT, ORIENT, OBSERVE, IGNORE]:
            a = self.act(env, action)
            b = self.act(fork, action)
            for item_a, item_b in zip(a[:3], b[:3]):
                self.assertTrue(torch.equal(item_a, item_b))
        env.restore(saved)
        resumed = env.clone()
        self.assertTrue(torch.equal(env.reset(), resumed.reset()))
        env.latent.fill_(1)
        self.assertFalse(torch.equal(env.latent, saved["latent"]))

    def test_strict_memory_recall_cannot_reveal_old_cue(self):
        a = self.environment("memory")
        b = ActiveInfoEnv(64, "cpu", 17, a.config)
        b.reset(latent_override=1 - a.latent)
        self.assertFalse(torch.equal(a.observation, b.observation))
        for t in range(1, a.episode_length):
            oa, _, _, _ = self.act(a, RECALL)
            ob, _, _, _ = self.act(b, RECALL)
            self.assertTrue(torch.equal(oa, ob))
        # Teacher retains the observed cue; current sensor stream has none.
        self.assertTrue((a.oracle_actions() != b.oracle_actions()).all())

    def test_bayes_teacher_does_not_know_unobserved_answers(self):
        env = self.environment(language_cost=1.2)
        while env.t < env.episode_length - 1:
            self.assertFalse((env.oracle_actions() == INVOKE_LANGUAGE).any())
            self.act(env, IGNORE)
        self.assertTrue((env.oracle_actions() == OBSERVE).all())
        self.assertGreater(int((env.latent == 1).sum()), 0)

    def test_zero_cost_single_factor_preserves_observations(self):
        charged = self.environment(mix_conditions=True)
        free = charged.clone()
        free.config["zero_call_cost"] = True
        a, ra, _, ia = self.act(charged, INVOKE_LANGUAGE)
        b, rb, _, ib = self.act(free, INVOKE_LANGUAGE)
        self.assertTrue(torch.equal(a, b))
        self.assertTrue(torch.equal(charged.resource, free.resource))
        self.assertTrue(torch.allclose(rb - ra, ia["language_cost"]))
        self.assertTrue((ib["language_cost"] == 0).all())

    def test_oracle_all_scenarios_and_decision_protocol(self):
        env = ActiveInfoEnv(512, seed=3)
        env.reset()
        for t in range(env.episode_length):
            self.assertTrue((env.observation[:, 14] == float(t == env.episode_length - 1)).all())
            _, _, done, info = env.step(env.oracle_actions())
            if t < env.episode_length - 1:
                self.assertFalse(info["success"].any())
        self.assertTrue(info["success"].all())
        self.assertTrue(done.all())
        with self.assertRaises(RuntimeError):
            self.act(env, IGNORE)

    def test_strict_memory_action_dependent_external_storage_blocked(self):
        base = self.environment("memory")
        branches = [base.clone() for _ in range(6)]
        for action, env in enumerate(branches):
            self.act(env, action)
        for _ in range(8):
            for env in branches[1:]:
                self.assertTrue(torch.equal(branches[0].observation, env.observation))
            for env in branches:
                self.act(env, IGNORE)
        # Physical expenditure differs, but cannot be read as external memory.
        self.assertFalse(torch.equal(branches[0].resource, branches[1].resource))

    def test_nonstrict_recall_replays_measured_value_not_truth(self):
        env = self.environment("memory", strict_memory=False, noise=.9)
        original = env.observation[:, 9].clone()
        self.act(env, IGNORE)
        self.act(env, RECALL)
        self.assertTrue(torch.equal(env.observation[:, 9], original))
        true_encoded = .25 + .5 * env.latent.float()
        self.assertFalse(torch.equal(env.observation[:, 9], true_encoded))

    def test_pending_latency_charge_cannot_be_avoided_by_ignore(self):
        base = self.environment(language_latency=4)
        ignore, wait = base.clone(), base.clone()
        self.act(ignore, INVOKE_LANGUAGE)
        self.act(wait, INVOKE_LANGUAGE)
        total = torch.zeros(64)
        for _ in range(4):
            _, ri, _, ii = self.act(ignore, IGNORE)
            _, rw, _, iw = self.act(wait, WAIT)
            self.assertTrue(torch.equal(ri, rw))
            total += ii["latency_cost"]
        self.assertTrue(torch.allclose(total, torch.full((64,), .008)))

    def test_external_mode_never_falls_back_to_scripted_truth(self):
        env = self.environment(language_backend="external", language_latency=0)
        self.act(env, INVOKE_LANGUAGE)
        self.assertTrue((env.observation[:, 10] == 0).all())
        self.assertTrue((env.language_fact == 0).all())
        for _ in range(3):
            self.act(env, WAIT)
            self.assertTrue((env.observation[:, 10] == 0).all())

    def test_external_latency_zero_correct_and_inverted(self):
        base = self.environment(language_backend="external", language_latency=0)
        a, b = base.clone(), base.clone()
        for env in (a, b):
            self.act(env, INVOKE_LANGUAGE)
        indices = torch.arange(64)
        a.inject_language_response(indices, a.latent, torch.ones(64), torch.ones(64, dtype=torch.bool))
        returned = b.inject_language_response(indices, 1 - b.latent, torch.ones(64), torch.ones(64, dtype=torch.bool))
        self.assertTrue((a.observation[:, 9] != b.observation[:, 9]).all())
        self.assertTrue(torch.equal(returned, b.observation))
        self.assertTrue(torch.equal(b.language_fact, 1 - b.latent))
        while a.t < a.episode_length - 1:
            self.act(a, IGNORE)
            self.act(b, IGNORE)
        self.assertTrue((a.oracle_actions() != b.oracle_actions()).all())

    def test_external_future_due_queue_unknown_and_restore(self):
        env = self.environment(language_backend="external", language_latency=2)
        self.act(env, INVOKE_LANGUAGE)
        valid = torch.arange(64) % 2 == 0
        env.inject_language_response(torch.arange(64), env.latent, torch.full((64,), .9), valid)
        self.assertTrue((env.observation[:, 10] == 0).all())
        fork = env.clone()
        for _ in range(2):
            self.act(env, WAIT)
            self.act(fork, WAIT)
        self.assertTrue(torch.equal(env.observation, fork.observation))
        self.assertTrue((env.observation[valid, 10] == .9).all())
        self.assertTrue((env.observation[~valid, 10] == 0).all())
        self.assertTrue(torch.equal(env.language_fact, env.latent))

    def test_external_rejects_precall_and_invalid_schema_atomically(self):
        env = self.environment(language_backend="external", language_latency=0)
        with self.assertRaisesRegex(ValueError, "before CALL"):
            env.inject_language_response([0], [1], [1.], [True])
        self.act(env, INVOKE_LANGUAGE)
        original = env.observation.clone()
        invalid = [([0], [2], [1.], [True]), ([0], [.5], [1.], [True]),
                   ([0], [1], [float("nan")], [True]), ([0], [1], [1.2], [True]),
                   ([0], [1], [1.], [1]), ([0, 0], [1, 1], [1., 1.], [True, True]),
                   ([65], [1], [1.], [True]), ([0], [1, 0], [1.], [True])]
        for args in invalid:
            with self.subTest(args=args), self.assertRaises(ValueError):
                env.inject_language_response(*args)
            self.assertTrue(torch.equal(env.observation, original))
            self.assertFalse(env.external_response_ready.any())
        # Caller converts invalid parser output to a valid unknown placeholder.
        env.inject_language_response([0], [0], [0.], [False])
        self.assertEqual(float(env.observation[0, 10]), 0.)

    def test_memory_delay_and_invalid_ood(self):
        env = self.environment("memory", memory_delay=640)
        self.assertEqual(env.episode_length, 641)
        with self.assertRaises(ValueError):
            self.environment(ood="typo")


if __name__ == "__main__":
    unittest.main()
