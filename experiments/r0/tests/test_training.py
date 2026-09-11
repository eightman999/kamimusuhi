import numpy as np
import torch

from ..agents.gates import build_policy
from ..env.r0_env import R0Config, VecR0Env
from ..train import collect_rollout, ppo_update


def cfg():
    return R0Config(episode_len=64, delay_min=8, delay_max=24,
                    num_distractors=6, queries_per_episode=2, seed=0)


def test_rollout_and_update_smoke():
    c = cfg()
    vec = VecR0Env(c, num_envs=8, seed=0)
    policy = build_policy("gru64", vec.obs_dim, c.num_values)
    opt = torch.optim.Adam(policy.parameters(), lr=3e-4)
    tc = dict(gamma=0.99, lam=0.95, clip=0.2, vf_coef=0.5, ent_coef=0.01,
              max_grad_norm=0.5, epochs=1, minibatch_episodes=8)
    bufs = collect_rollout(policy, vec, torch.device("cpu"))
    assert bufs[0].shape == (8, 64, vec.obs_dim)
    assert len(bufs[6]) == 8                       # one ep_stats per env
    losses = ppo_update(policy, opt, bufs[:6], tc, torch.device("cpu"))
    assert np.isfinite(losses["pi_loss"]) and np.isfinite(losses["v_loss"])
