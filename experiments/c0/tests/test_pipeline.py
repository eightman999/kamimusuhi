import json
import subprocess
import sys
from dataclasses import asdict
from pathlib import Path

import numpy as np
import pytest
import torch

from ..agents.heuristics import HeuristicRunner
from ..agents.policies import build_policy
from ..agents.runner import Runner
from ..config_util import load_env_cfg
from ..env.c0_env import C0Config, C0Env
from ..persistence.codec import state_spec_from
from ..persistence.interrupt import run_interrupted

ROOT = Path(__file__).resolve().parents[3]
TINY = ["episode_len=48", "stops=20:0", "n_pre=0", "n_bridge=1", "n_ctrl=1",
        "post_q_lo=6", "ctrl_item_hi=16", "ctrl_delay_min=6", "item_lo=4",
        "item_hi_margin=4", "num_distractors=4", "query_window=3",
        "memory_slots=8", "noise_rate=0.3"]


def _gru_runner(env_cfg):
    torch.manual_seed(0)
    pol = build_policy("gru64", env_cfg)
    return Runner(pol, "gru64", env_cfg)


def test_full_restore_equals_uninterrupted():
    """Full state restore must reproduce the uninterrupted trajectory."""
    env_cfg, _ = load_env_cfg("experiments/c0/configs/default.yaml", TINY)
    spec = state_spec_from(["hidden"], env_cfg, 64)
    seeds = [101, 202, 303]
    for cond, kwargs in (("uninterrupted", {"uninterrupted": True}),
                         ("full", {})):
        envs = [C0Env(env_cfg) for _ in seeds]
        for e, s in zip(envs, seeds):
            e.reset(seed=s)
        res = run_interrupted(_gru_runner(env_cfg), envs, spec,
                              greedy=True, **kwargs)
        if cond == "uninterrupted":
            rec_un = res["records"]
        else:
            rec_full = res["records"]
    for a, b in zip(rec_un, rec_full):
        assert [(q["t_query"], q["correct"]) for q in a] == \
               [(q["t_query"], q["correct"]) for q in b]


def test_cold_breaks_post_not_ctrl():
    """Dependency check: storeall wiped at the stop must fail post queries
    while still answering ctrl queries (post-restart obs alone is not
    enough)."""
    env_cfg, _ = load_env_cfg("experiments/c0/configs/default.yaml", [])
    spec = state_spec_from(["mem_payloads", "mem_keys", "mem_occupied",
                            "mem_insert", "mem_clock"], env_cfg, 0)
    for cond, want in (("full", 0.8), ("cold", 0.35)):
        envs = [C0Env(env_cfg) for _ in range(40)]
        for i, e in enumerate(envs):
            e.reset(seed=1000 + i)
        r = HeuristicRunner("storeall", env_cfg, seed=5)
        res = run_interrupted(r, envs, spec, condition=cond, greedy=True)
        rows = [q for rec in res["records"] for q in rec]
        post = [q["correct"] for q in rows if q["cls"] == "post"]
        ctrl = [q["correct"] for q in rows if q["cls"] == "ctrl"]
        if cond == "full":
            assert np.mean(post) > want
        else:
            assert np.mean(post) < want          # fails post
        assert np.mean(ctrl) > 0.8               # still works post-restart


def test_wrong_state_degrades_inprocess():
    env_cfg, _ = load_env_cfg("experiments/c0/configs/default.yaml", TINY)
    spec = state_spec_from(["mem_payloads", "mem_keys", "mem_occupied",
                            "mem_insert", "mem_clock"], env_cfg, 0)
    envs = [C0Env(env_cfg) for _ in range(30)]
    for i, e in enumerate(envs):
        e.reset(seed=2000 + i)
    donors = [C0Env(env_cfg) for _ in range(30)]
    for i, e in enumerate(donors):
        e.reset(seed=9000 + i)
    # collect donor payloads by running donors to the stop
    from ..persistence.interrupt import run_leg, capture_payloads
    dr = HeuristicRunner("storeall", env_cfg, seed=7)
    dr.reset(30)
    run_leg(donors, dr, env_cfg.stops[0][0])
    donor_payloads = capture_payloads(dr, "full", None, spec)
    r = HeuristicRunner("storeall", env_cfg, seed=5)
    res = run_interrupted(r, envs, spec, condition="full",
                          donor_payloads=donor_payloads, greedy=True)
    rows = [q for rec in res["records"] for q in rec]
    post = [q["correct"] for q in rows if q["cls"] == "post"]
    assert np.mean(post) < 0.35


@pytest.mark.slow
def test_subprocess_boundary(tmp_path):
    """Real process boundary smoke: phase A writes artifact, exits; phase B
    continues the same episode. Also checks A+B == in-process result."""
    env_cfg, env_d = load_env_cfg("experiments/c0/configs/default.yaml", TINY)
    torch.manual_seed(0)
    pol = build_policy("mem64", env_cfg)
    ckpt = tmp_path / "ck.pt"
    torch.save({"model": pol.state_dict(), "model_name": "mem64",
                "env": env_d, "seed": 0}, ckpt)
    art = tmp_path / "art.json"
    seg_a, seg_b = tmp_path / "a.json", tmp_path / "b.json"
    base = ["--config", "experiments/c0/configs/default.yaml",
            "--episodes", "4", "--ep-seed-base", "6000"]
    r1 = subprocess.run(
        [sys.executable, "-m", "experiments.c0.run_phase_a",
         "--checkpoint", str(ckpt), "--condition", "full",
         "--artifact-out", str(art), "--metrics-out", str(seg_a)]
        + base + ["--set"] + TINY, cwd=ROOT, capture_output=True, text=True)
    assert r1.returncode == 0, r1.stderr
    assert art.exists()
    r2 = subprocess.run(
        [sys.executable, "-m", "experiments.c0.run_phase_b",
         "--checkpoint", str(ckpt), "--artifact-in", str(art),
         "--stop-index", "0", "--metrics-out", str(seg_b)]
        + base + ["--set"] + TINY, cwd=ROOT, capture_output=True, text=True)
    assert r2.returncode == 0, r2.stderr
    ma = json.loads(seg_a.read_text())
    mb = json.loads(seg_b.read_text())
    assert len(mb["episodes"]) == 4
    tot_q = sum(len(e["queries"]) for e in ma["episodes"]) + \
        sum(len(e["queries"]) for e in mb["episodes"])
    assert tot_q == 8                    # 2 queries per episode x 4

    # equivalence with the in-process engine
    spec = state_spec_from(["hidden", "mem_payloads", "mem_keys",
                            "mem_occupied", "mem_insert", "mem_clock"],
                           env_cfg, 64)
    envs = [C0Env(env_cfg) for _ in range(4)]
    for i, e in enumerate(envs):
        e.reset(seed=6000 + i)
    pol2 = build_policy("mem64", env_cfg)
    pol2.load_state_dict(torch.load(ckpt, weights_only=False)["model"])
    res = run_interrupted(Runner(pol2, "mem64", env_cfg), envs, spec,
                          condition="full", greedy=True)
    inproc = [q for rec in res["records"] for q in rec]
    subp = [q for e in ma["episodes"] for q in e["queries"]] + \
           [q for e in mb["episodes"] for q in e["queries"]]
    # order within an episode is chronological; compare as multisets per ep
    for i in range(4):
        a = [(q["t_query"], q["correct"]) for q in inproc if True]
        ip = [(q["t_query"], q["correct"]) for q in res["records"][i]]
        sp = [(q["t_query"], q["correct"])
              for q in (ma["episodes"][i]["queries"]
                        + mb["episodes"][i]["queries"])]
        assert sorted(ip) == sorted(sp)
