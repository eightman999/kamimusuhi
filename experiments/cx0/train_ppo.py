"""CX0 PPO fine-tune — Stage B2 after BC bootstrap.

BC teaches the action distribution but cannot punish out-of-window responses;
the task reward (success +1, wrong-window RESP −0.2, death −1) can. Standard
clipped PPO over replayed recurrent rollouts (initial hidden state per
episode, BPTT through time), plus small aux next-sensory loss and entropy
bonus. Pattern follows experiments/r0 and experiments/u0.

Usage:
  python -m experiments.cx0.train_ppo --arm c3 --task ctx3 --seed 0 \
      --iters 60 --eps 32 --out experiments/cx0/runs
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

import dataclasses

from .env.ctx_world import EP_LEN, TASKS
from .models.arms import ARMS, build_arm, count_params
from .organs.pretrain import build_organ_set
from .runner import Intervention, rollout
from .train_bc import CKPT_ORGANS, eval_success

GAMMA, LAM = 0.97, 0.95


def collect_batch(task, seed0, organs, arm, n_eps, rng, device="cpu",
                  spec=None):
    eps = []
    for i in range(n_eps):
        tr = rollout(task, seed=seed0 + i, organs=organs, arm=arm,
                     sample=True, record_pops=False, rng=rng, spec=spec)
        eps.append(tr.arrays() | {"values": np.array(tr.values),
                                  "logps": np.array(tr.logps)})
    return eps


def gae(eps):
    """Per-episode GAE -> padded (T,B) tensors."""
    B = len(eps)
    T = max(len(e["actions"]) for e in eps)
    bundles = np.zeros((T, B, eps[0]["bundles"].shape[1]), dtype=np.float32)
    acts = np.zeros((T, B), dtype=np.int64)
    adv = np.zeros((T, B), dtype=np.float32)
    ret = np.zeros((T, B), dtype=np.float32)
    oldlogp = np.zeros((T, B), dtype=np.float32)
    nexts = np.zeros((T, B, 16), dtype=np.float32)
    mask = np.zeros((T, B), dtype=np.float32)
    for j, e in enumerate(eps):
        n = len(e["actions"])
        r = e["rewards"]; v = e["values"]
        a = np.zeros(n, dtype=np.float32)
        last = 0.0
        for t in range(n - 1, -1, -1):
            nv = v[t + 1] if t + 1 < n else 0.0
            d = r[t] + GAMMA * nv - v[t]
            last = d + GAMMA * LAM * last
            a[t] = last
        bundles[:n, j] = e["bundles"]
        acts[:n, j] = e["actions"]
        adv[:n, j] = a
        ret[:n, j] = a + v
        oldlogp[:n, j] = e["logps"]
        nexts[:n, j] = e["next_sensory"]
        mask[:n, j] = 1.0
    return (torch.tensor(bundles), torch.tensor(acts), torch.tensor(adv),
            torch.tensor(ret), torch.tensor(oldlogp), torch.tensor(nexts),
            torch.tensor(mask))


def ppo_update(arm, opt, eps, class_w=None, clip=0.2, ent_w=0.01,
               val_w=0.5, aux_w=0.1, epochs=4, mb_eps=16, rng=None):
    rng = rng or np.random.default_rng(0)
    stats = {}
    for _ in range(epochs):
        order = rng.permutation(len(eps))
        for lo in range(0, len(order), mb_eps):
            batch = [eps[i] for i in order[lo:lo + mb_eps]]
            bundles, acts, adv, ret, oldlogp, nexts, mask = gae(batch)
            adv = (adv - adv.sum() / mask.sum().clamp(min=1))
            T, B = bundles.shape[0], bundles.shape[1]
            state = arm.initial_state(B)
            loss = 0.0
            for t in range(T):
                logits, v, aux, state, _p = arm(bundles[t], state)
                dist = torch.log_softmax(logits, -1)
                logp = dist.gather(1, acts[t][:, None]).squeeze(1)
                ratio = torch.exp(logp - oldlogp[t])
                s1 = ratio * adv[t]
                s2 = torch.clamp(ratio, 1 - clip, 1 + clip) * adv[t]
                m = mask[t]
                pol = -(torch.min(s1, s2) * m).sum() / m.sum().clamp(min=1)
                vloss = (((v - ret[t]) ** 2) * m).sum() / m.sum().clamp(min=1)
                ent = -(dist.exp() * dist).sum(-1)
                ent = (ent * m).sum() / m.sum().clamp(min=1)
                auxl = (((aux - nexts[t]) ** 2).mean(-1) * m).sum() \
                    / m.sum().clamp(min=1)
                loss = loss + pol + val_w * vloss - ent_w * ent + aux_w * auxl
            opt.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(arm.parameters(), 5.0)
            opt.step()
            stats = dict(loss=float(loss.detach()), pol=float(pol.detach()),
                         vloss=float(vloss.detach()), ent=float(ent.detach()))
    return stats


def run(task, arm_name, seed, iters, n_eps, out_dir, organ_dir, device="cpu",
        lr=1e-3, init_from_bc=True):
    torch.manual_seed(seed)
    rng = np.random.default_rng(seed + 5000)
    organs = build_organ_set(organ_dir, device=device)
    out = Path(out_dir) / task / arm_name / f"s{seed}"
    bc_ckpt = out / "best.pt"
    arm = build_arm(arm_name)
    if init_from_bc and bc_ckpt.exists():
        arm.load_state_dict(torch.load(bc_ckpt, weights_only=True)["arm"])
    opt = torch.optim.Adam(arm.parameters(), lr=lr)
    val_seeds = [700001 + 1000 * k for k in range(8)]
    best = -1.0
    log = []
    for it in range(iters):
        # disclosed curriculum (u0-style): early iters narrow the delay range
        spec = TASKS[task]
        if task == "ctx3" and it < iters // 2:
            spec = dataclasses.replace(spec, resp_delay_range=(16, 40))
        eps = collect_batch(task, 300000 + seed * 1000 + it * 64, organs, arm,
                            n_eps, rng, device, spec=spec)
        stats = ppo_update(arm, opt, eps, rng=rng)
        if it % 5 == 0 or it == iters - 1:
            val, vagree = eval_success(task, val_seeds, organs, arm)
            rec = dict(it=it, val_success=val, val_probe_agree=vagree, **stats)
            log.append(rec)
            print(f"[{task}/{arm_name}/s{seed}] it{it} val={val:.3f} "
                  f"loss={stats['loss']:.3f} ent={stats['ent']:.3f}",
                  flush=True)
            if val >= best:
                best = val
                torch.save({"arm": arm.state_dict(), "arm_name": arm_name,
                            "task": task, "seed": seed, "val_success": val,
                            "n_params": count_params(arm),
                            "stage": "ppo"}, out / "ppo_best.pt")
            (out / "ppo_metrics.jsonl").write_text(
                "\n".join(json.dumps(x) for x in log))
    return best


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arm", required=True, choices=list(ARMS))
    ap.add_argument("--task", required=True)
    ap.add_argument("--seeds", type=int, nargs="+", default=[0])
    ap.add_argument("--iters", type=int, default=60)
    ap.add_argument("--eps", type=int, default=32)
    ap.add_argument("--out", default=str(CKPT_ORGANS.parent))
    ap.add_argument("--organ-dir", default=str(CKPT_ORGANS))
    ap.add_argument("--lr", type=float, default=1e-3)
    ap.add_argument("--scratch", action="store_true")
    a = ap.parse_args()
    t0 = time.time()
    for s in a.seeds:
        run(a.task, a.arm, s, a.iters, a.eps, a.out, a.organ_dir,
            init_from_bc=not a.scratch)
    print(f"done in {time.time() - t0:.1f}s")


if __name__ == "__main__":
    main()
