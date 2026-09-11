import numpy as np
import torch

from ..agents.baselines import BASELINES, FIFOGate, OracleGate
from ..agents.gates import build_policy, factorized_logp
from ..env.r0_env import R0Config, R0Env, ITEM, QUERY
from ..evaluate import evaluate_baseline, evaluate_learned


def cfg(**kw):
    base = dict(episode_len=96, delay_min=8, delay_max=32, num_distractors=8,
                queries_per_episode=3, seed=7)
    base.update(kw)
    return R0Config(**base)


def test_policy_shapes():
    c = cfg()
    for name in ["mlp", "gru64", "gru128"]:
        p = build_policy(name, c.obs_dim, c.num_values)
        h = p.initial_state(4, torch.device("cpu"))
        al, nl, v, h2 = p(torch.zeros(4, c.obs_dim), h)
        assert al.shape == (4, 4) and nl.shape == (4, c.num_values)
        assert v.shape == (4,)


def test_factorized_logp_counts_answer_only_on_answer():
    c = cfg()
    p = build_policy("mlp", c.obs_dim, c.num_values)
    al, nl, _v, _ = p(torch.zeros(2, c.obs_dim))
    act = torch.tensor([3, 0])
    ans = torch.tensor([1, 1])
    logp, is_ans = factorized_logp(al, nl, act, ans)
    assert is_ans.tolist() == [1.0, 0.0]
    only_act = torch.distributions.Categorical(logits=al).log_prob(act)
    assert torch.allclose(logp[1], only_act[1])


def test_oracle_beats_fifo():
    c = cfg()
    oracle = evaluate_baseline("oracle", c, episodes=24, seed=900001)
    fifo = evaluate_baseline("fifo", c, episodes=24, seed=900001)
    assert oracle["accuracy"] > 0.95
    assert oracle["accuracy"] > fifo["accuracy"]
    assert oracle["store_precision"] > 0.99


def test_erase_causal_op_hurts_baseline():
    c = cfg()
    clean = evaluate_baseline("oracle", c, episodes=24, seed=900001)
    erased = evaluate_baseline("oracle", c, episodes=24, seed=900001,
                             causal="erase")
    assert clean["accuracy"] > 0.95
    assert erased["accuracy"] < 0.4


def test_permute_preserves_baseline():
    c = cfg()
    clean = evaluate_baseline("oracle", c, episodes=24, seed=900001)
    perm = evaluate_baseline("oracle", c, episodes=24, seed=900001,
                           causal="permute")
    assert abs(clean["accuracy"] - perm["accuracy"]) < 0.01
