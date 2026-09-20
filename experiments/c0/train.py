"""Imitation trainer for C0.

Teacher-forced imitation of the oracle (ANSWER true value at QUERY steps,
STORE every ITEM for memory agents, IGNORE otherwise). Whole-sequence BPTT:
each iteration rolls out fresh episodes and re-runs them through the model.

Restart augmentation: with probability `reset_prob`, the runner's mutable
state is reset mid-episode at a random step, in a random mode
("all"=cold, "hidden"=hidden-only restore, "memory"=memory-only restore),
so every restore condition evaluated later is in-distribution.

Auxiliary binding loss: the answer head is additionally supervised on
ITEM steps and the following `aux_span` steps (target = the item's value),
which gives the GRU a constant retention gradient, not only at queries.

Budget controls (env vars): C0_TIME_BUDGET, C0_MAX_ITERS, C0_NUM_ENVS.

    python -m experiments.c0.train --config experiments/c0/configs/default.yaml \
        --model gru64 --seed 0 --artifacts experiments/c0/artifacts --run-id g64_s0
"""
from __future__ import annotations

import argparse
import json
import os
import time
from dataclasses import asdict
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

from .agents.policies import MODEL_SPECS, build_policy
from .agents.runner import Runner
from .config_util import git_commit, load_env_cfg, load_full_cfg, tensor_hash
from .env.c0_env import C0Env, C0Config, ITEM, QUERY, ANSWER, STORE
from .persistence.codec import state_spec_from
from .persistence.interrupt import run_interrupted


def save_ckpt(path, policy, model_name, ecfg, tc, seed, iters, t0, val):
    torch.save({"model": policy.state_dict(), "model_name": model_name,
                    "env": asdict(ecfg), "train": tc, "seed": seed,
                    "git": git_commit(), "iters": iters,
                    "wall_time": round(time.time() - t0, 1),
                    "val": val,
                    "model_sha256": tensor_hash(policy)},
                   Path(path))


def validate(policy, model_name, ecfg, device, episodes, seed):
    """Greedy uninterrupted validation; per-class accuracy."""
    envs = [C0Env(C0Config(**{**asdict(ecfg), "seed": seed + i}))
            for i in range(episodes)]
    for e in envs:
        e.reset()
    runner = Runner(policy, model_name, ecfg, device)
    spec = state_spec_from(runner.state_fields(), ecfg,
                           getattr(policy, "hidden", 0))
    res = run_interrupted(runner, envs, spec, uninterrupted=True)
    stats = res["stats"]
    keys = ["accuracy", "acc_pre", "acc_post", "acc_ctrl"]
    return {k: float(np.nanmean([s[k] for s in stats])) for k in keys}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--model", required=True, choices=sorted(MODEL_SPECS))
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--artifacts", default="experiments/c0/artifacts")
    ap.add_argument("--run-id", default=None)
    ap.add_argument("--device", default="cpu")
    args = ap.parse_args()

    full = load_full_cfg(args.config)
    ecfg, env_d = load_env_cfg(args.config)
    ecfg = C0Config(**{**asdict(ecfg), "seed": args.seed})
    tc = full["train"]
    iters = int(os.environ.get("C0_MAX_ITERS", tc["iters"]))
    num_envs = int(os.environ.get("C0_NUM_ENVS", tc["num_envs"]))
    budget = float(os.environ.get("C0_TIME_BUDGET", "0"))

    run_id = args.run_id or f"{args.model}_s{args.seed}_{int(time.time())}"
    run_dir = Path(args.artifacts) / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)

    torch.manual_seed(args.seed)
    np.random.seed(args.seed)
    rng = np.random.default_rng(args.seed)
    device = torch.device(args.device)
    policy = build_policy(args.model, ecfg).to(device)
    opt = torch.optim.Adam(policy.parameters(), lr=tc["lr"])
    proto = Runner(policy, args.model, ecfg, device)

    (run_dir / "config.json").write_text(json.dumps(
        {"env": env_d, "train": tc, "model": args.model, "seed": args.seed,
         "run_id": run_id, "git": git_commit()}, indent=2))
    metrics_f = open(run_dir / "metrics.jsonl", "a")
    save_ckpt(run_dir / "initial.pt", policy, args.model, ecfg, tc,
              args.seed, 0, time.time(), {})

    envs = [C0Env(C0Config(**{**asdict(ecfg), "seed": 0}))
            for _ in range(num_envs)]
    has_mem = MODEL_SPECS[args.model]["memory"]
    modes = ["all", "hidden", "memory"] if has_mem else ["all", "hidden"]

    best = -1.0
    val = {}
    t0 = time.time()
    for it in range(1, iters + 1):
        for i, e in enumerate(envs):
            e.cfg.seed = args.seed * 1_000_003 + it * 977 + i
        # curriculum: slide the interval-0 item window toward the first stop
        # so early iterations only need short retention spans
        warm = tc.get("warmup_iters", 0)
        if warm:
            f = min(1.0, it / warm)
            stop0 = ecfg.stops[0][0]
            lo_full = full.get("env", {}).get("item_lo", 6)
            lo_eff = round(stop0 - ecfg.item_hi_margin
                           - f * (stop0 - ecfg.item_hi_margin - lo_full))
            for e in envs:
                e.cfg.item_lo = max(2, lo_eff)
        # restart augmentation: pick a mode + step for this iteration
        do_reset = rng.random() < tc.get("reset_prob", 0.6)
        mode = modes[int(rng.integers(len(modes)))]
        reset_t = int(rng.integers(ecfg.episode_len // 4,
                                   3 * ecfg.episode_len // 4))
        losses = train_iter(policy, proto, envs, opt, device, tc,
                            do_reset, mode, reset_t, rng)
        rec = {"iter": it, "elapsed": round(time.time() - t0, 1), **losses,
               "reset_mode": mode if do_reset else "none",
               "reset_t": reset_t if do_reset else -1}
        if it % tc["eval_every"] == 0 or it == iters:
            val = validate(policy, args.model, ecfg, device,
                           tc["eval_episodes"], tc["val_seed"])
            rec.update({f"val_{k}": v for k, v in val.items()})
            if val["accuracy"] > best:
                best = val["accuracy"]
                save_ckpt(run_dir / "best.pt", policy, args.model, ecfg,
                          tc, args.seed, it, t0, val)
        metrics_f.write(json.dumps(rec) + "\n")
        metrics_f.flush()
        print(f"[{run_id}] it {it} act {losses['act_loss']:.3f} "
              f"q {losses['q_loss']:.3f} x {losses['x_loss']:.3f} "
              f"val {rec.get('val_accuracy', float('nan')):.3f} "
              f"post {rec.get('val_acc_post', float('nan')):.3f}",
              flush=True)
        if budget and time.time() - t0 > budget:
            break

    save_ckpt(run_dir / "final.pt", policy, args.model, ecfg, tc,
              args.seed, it, t0, val)
    (run_dir / "done.json").write_text(json.dumps(
        {"iters": it, "best_val_accuracy": best,
         "elapsed_sec": time.time() - t0}, indent=2))
    print(f"[{run_id}] done iters={it} best_val={best:.3f}")


def train_iter(policy, proto, envs, opt, device, tc,
               do_reset, mode, reset_t, rng):
    """One teacher-forced rollout + sequence CE update."""
    T = envs[0].cfg.episode_len
    N = len(envs)
    for e in envs:
        e.reset()
        e._last_item_t, e._last_item_v = -10**9, -1
    proto.reset(N)
    al_t, nl_t, acts_t, anss_t, qmask_t, xmask_t = [], [], [], [], [], []
    aux = tc.get("aux_span", 12)
    for t in range(T):
        if do_reset and t == reset_t:
            proto.reset_where(np.arange(N), mode)
        x = proto._input(np.stack([e._obs() for e in envs]))
        al, nl, _v, proto.h = policy(x, proto.h)
        al_t.append(al); nl_t.append(nl)
        oa, oan, qm, xm = [], [], [], []
        for i, e in enumerate(envs):
            a, v = e.oracle_action(store_items=proto.has_memory)
            oa.append(a); oan.append(v)
            ev = e.schedule[e.t]
            if ev.kind == QUERY:
                qm.append(1); xm.append(0)          # real retention signal
            else:
                qm.append(0)
                if ev.kind == ITEM:
                    e._last_item_t, e._last_item_v = e.t, ev.value
                    xm.append(1)
                elif e.t - e._last_item_t <= aux:
                    oan[i] = e._last_item_v
                    xm.append(1)
                else:
                    xm.append(0)
        acts_t.append(torch.as_tensor(oa, device=device))
        anss_t.append(torch.as_tensor(oan, device=device))
        qmask_t.append(torch.as_tensor(qm, dtype=torch.float32,
                                       device=device))
        xmask_t.append(torch.as_tensor(xm, dtype=torch.float32,
                                       device=device))
        for i, e in enumerate(envs):
            if oa[i] == STORE and proto.has_memory:
                ev = e.schedule[e.t]
                proto.mems[i].store(e.event_payload(), ev.key)
            e.step(int(oa[i]), int(oan[i]))
    AL = torch.stack(al_t, 1)          # (N, T, A)
    NL = torch.stack(nl_t, 1)
    A = torch.stack(acts_t, 1)
    AV = torch.stack(anss_t, 1)
    MQ = torch.stack(qmask_t, 1)
    MX = torch.stack(xmask_t, 1)
    act_loss = nn.functional.cross_entropy(
        AL.reshape(-1, AL.shape[-1]), A.reshape(-1))
    ce = nn.functional.cross_entropy(
        NL.reshape(-1, NL.shape[-1]), AV.reshape(-1), reduction="none")
    q_loss = (ce * MQ.reshape(-1)).sum() / (MQ.sum() + 1e-8)
    x_loss = (ce * MX.reshape(-1)).sum() / (MX.sum() + 1e-8)
    ans_loss = tc.get("ans_coef", 4.0) * q_loss + tc.get("aux_coef", 0.3) * x_loss
    loss = act_loss + ans_loss
    opt.zero_grad()
    loss.backward()
    nn.utils.clip_grad_norm_(policy.parameters(), tc["max_grad_norm"])
    opt.step()
    return {"act_loss": float(act_loss.item()),
            "ans_loss": float(ans_loss.item()),
            "q_loss": float(q_loss.item()), "x_loss": float(x_loss.item()),
            "q_mask": float(MQ.mean().item()), "x_mask": float(MX.mean().item())}


if __name__ == "__main__":
    main()
