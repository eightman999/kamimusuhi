import numpy as np

from env import AgencyEnv, EnvConfig
from env.dynamics import CauseLabels, N_ACTIONS, NOOP


def test_deterministic_replay():
    a = AgencyEnv(EnvConfig(), seed=3)
    b = AgencyEnv(EnvConfig(), seed=3)
    rng = np.random.default_rng(0)
    acts = rng.integers(N_ACTIONS, size=32)
    oa = [a.reset()]
    ob = [b.reset()]
    for act in acts:
        oa.append(a.step(int(act))[0])
        ob.append(b.step(int(act))[0])
    np.testing.assert_allclose(np.stack(oa), np.stack(ob))


def test_action_only_moves_self_and_mixed_dims():
    env = AgencyEnv(EnvConfig(), seed=1)
    env.reset()
    rng = np.random.default_rng(7)
    for _ in range(20):
        st = env.get_state()
        a = int(rng.integers(N_ACTIONS - 1))  # non-NOOP
        env.set_state(st)
        o_a, _ = env.step(a)
        env.set_state(st)
        o_n, _ = env.step(NOOP)
        diff = np.abs(o_a - o_n)
        lab = env.cause_labels
        selfish = np.isin(lab, [CauseLabels.SELF, CauseLabels.MIXED])
        assert diff[selfish].sum() > 1e-6
        np.testing.assert_allclose(diff[~selfish], 0.0, atol=1e-12)
        env.set_state(st)
        env.step(NOOP)


def test_noop_leaves_zero_action_component():
    env = AgencyEnv(EnvConfig(), seed=2)
    env.reset()
    _, info = env.step(NOOP)
    np.testing.assert_allclose(info.action_component, 0.0, atol=1e-12)


def test_decomposition_sums_to_obs():
    env = AgencyEnv(EnvConfig(), seed=4)
    env.reset()
    rng = np.random.default_rng(1)
    for _ in range(16):
        obs, info = env.step(int(rng.integers(N_ACTIONS)))
        np.testing.assert_allclose(
            info.world_component + info.action_component
            + info.noise_component, obs, atol=1e-10)


def test_disturbance_hits_only_external_and_mixed():
    cfg = EnvConfig(disturbance_prob=1.0, disturbance_gain=5.0,
                    mix_disturbance_gain=5.0)
    env = AgencyEnv(cfg, seed=5)
    env.reset()
    rng = np.random.default_rng(3)
    for _ in range(10):
        _, info = env.step(int(rng.integers(N_ACTIONS)))
        lab = env.cause_labels
        ext_m = np.isin(lab, [CauseLabels.EXTERNAL, CauseLabels.MIXED])
        # disturbance shows up inside world_component on ext/mix dims
        assert np.abs(info.world_component[ext_m]).mean() > 0
        np.testing.assert_allclose(
            info.action_component[lab == CauseLabels.EXTERNAL],
            0.0, atol=1e-12)


def test_action_delay():
    cfg = EnvConfig(action_delay=2, process_noise=0.0, obs_noise=0.0)
    env = AgencyEnv(cfg, seed=6)
    env.reset()
    st = env.get_state()
    env.set_state(st)
    o_a, _ = env.step(0)
    env.set_state(st)
    o_n, _ = env.step(NOOP)
    np.testing.assert_allclose(o_a, o_n, atol=1e-12)  # not landed yet


def test_state_roundtrip():
    env = AgencyEnv(EnvConfig(), seed=8)
    env.reset()
    for a in [0, 2, 1]:
        env.step(a)
    st = env.get_state()
    o1, _ = env.step(0)
    env.set_state(st)
    o2, _ = env.step(0)
    np.testing.assert_allclose(o1, o2)


def test_sensor_coupling_leaks_across_dims():
    base = EnvConfig(sensor_coupling=0.0, process_noise=0.0, obs_noise=0.0,
                     noise_dim_std=0.0)
    env0 = AgencyEnv(base, seed=9)
    env1 = AgencyEnv(EnvConfig(**{**base.__dict__, "sensor_coupling": 0.5}),
                     seed=9)
    env0.reset(); env1.reset()
    rng = np.random.default_rng(0)
    diffs = []
    for _ in range(10):
        a = int(rng.integers(N_ACTIONS))
        diffs.append(env0.step(a)[0] - env1.step(a)[0])
    assert np.abs(np.stack(diffs)).max() > 0
