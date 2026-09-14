"""Stage B — integration training by behavior cloning (DAGGER-style mixing).

Student trajectories are rolled out under the arm's own actions with prob
(1-beta) and the oracle's with prob beta (annealed across rounds); every
visited state is labeled with the oracle's action. Loss = CE(policy) +
0.5 * MSE(aux next-sensory). Best checkpoint is selected on a fixed
validation grid by pure-student success rate.

Usage:
  python -m experiments.cx0.train_bc --arm c3 --task ctx1 --seed 0 \
      --rounds 6 --episodes 64 --out experiments/cx0/runs
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

from .env.ctx_world import EP_LEN, N_ACTIONS, Oracle
from .models.arms import ARMS, build_arm, count_params
from .organs.pretrain import build_organ_set
from .runner import Intervention, rollout

CKPT_ORGANS = Path(__file__).parent / "runs" / "organs"
BETA0, BETA_FLOOR, BETA_DECAY = 0.9, 0.2, 0.06
BUFFER_CAP = 480


def collect_round(task, seed, organs, arm, oracle, beta, n_eps, rng):
    """`seed` must differ per round — callers pass seed*1000+round."""
    data = []
    for i in range(n_eps):
        tr = rollout(task, seed=seed * 100003 + i, organs=organs, arm=arm,
                     oracle=oracle, mix_beta=beta, record_pops=False, rng=rng)
        arr = tr.arrays()
        data.append(arr)
    return data


def _pad_batch(data, idxs):
    """Pack episodes into (T,B,*) tensors with a validity mask."""
    T = max(len(d["actions"]) for d in (data[i] for i in idxs))
    B = len(idxs)
    bundles = np.zeros((T, B, data[0]["bundles"].shape[1]), dtype=np.float32)
    labels = np.zeros((T, B), dtype=np.int64)
    nexts = np.zeros((T, B, 16), dtype=np.float32)
    mask = np.zeros((T, B), dtype=np.float32)
    for j, i in enumerate(idxs):
        arr = data[i]
        n = len(arr["actions"])
        bundles[:n, j] = arr["bundles"]
        labels[:n, j] = np.clip(arr["oracle_actions"], 0, N_ACTIONS - 1)
        nexts[:n, j] = arr["next_sensory"]
        valid = arr["oracle_actions"] >= 0
        mask[:n, j] = valid.astype(np.float32)
    return (torch.tensor(bundles), torch.tensor(labels),
            torch.tensor(nexts), torch.tensor(mask))


def train_epochs(arm, data, opt, class_w, n_epochs=3, aux_w=0.1, rng=None,
                 batch_eps: int = 64):
    """Batched BPTT: all episodes stepped in lockstep (cell-based arms)."""
    rng = rng or np.random.default_rng(0)
    tot = 0.0
    n = 0
    for _ in range(n_epochs):
        order = rng.permutation(len(data))
        for lo in range(0, len(order), batch_eps):
            idxs = order[lo:lo + batch_eps]
            bundles, labels, nexts, mask = _pad_batch(data, idxs)
            T, B = bundles.shape[0], bundles.shape[1]
            state = arm.initial_state(B)
            loss = 0.0
            for t in range(T):
                logits, _v, aux, state, _pops = arm(bundles[t], state)
                ce = nn.functional.cross_entropy(
                    logits, labels[t], weight=class_w, reduction="none")
                loss = loss + (ce * mask[t]).sum() / mask[t].sum().clamp(min=1)
                loss = loss + aux_w * (
                    ((aux - nexts[t]) ** 2).mean(-1) * mask[t]).sum() \
                    / mask[t].sum().clamp(min=1)
            opt.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(arm.parameters(), 5.0)
            opt.step()
            tot += float(loss.detach()); n += 1
    return tot / max(n, 1)


@torch.no_grad()
def eval_success(task, seeds, organs, arm):
    wins = 0
    agree = []
    for i, s in enumerate(seeds):
        tr = rollout(task, seed=s + i, organs=organs, arm=arm, oracle=None,
                     record_pops=False)
        wins += int(tr.success)
        if tr.probe_agree is not None:
            agree.append(tr.probe_agree)
    return wins / max(len(seeds), 1), float(np.mean(agree)) if agree else None


def run(task, arm_name, seed, rounds, n_eps, out_dir, organ_dir,
        lr=3e-3, epochs_per_round=3, device="cpu"):
    torch.manual_seed(seed)
    rng = np.random.default_rng(seed)
    organs = build_organ_set(organ_dir, device=device)
    arm = build_arm(arm_name)
    opt = torch.optim.Adam(arm.parameters(), lr=lr)
    oracle = Oracle(task)
    out = Path(out_dir) / task / arm_name / f"s{seed}"
    out.mkdir(parents=True, exist_ok=True)

    val_seeds = [700001 + 1000 * k for k in range(8)]
    data: list = []
    best = -1.0
    log = []
    for r in range(rounds):
        beta = max(BETA_FLOOR, BETA0 - BETA_DECAY * r)
        data += collect_round(task, seed * 1000 + r, organs, arm, oracle,
                              beta, n_eps, rng)
        if len(data) > BUFFER_CAP:
            data = data[-BUFFER_CAP:]
        # mild class balancing: sqrt-inverse freq, clipped (full inverse-freq
        # weights overfire rare RESP actions into degenerate policies)
        labels = np.concatenate([d["oracle_actions"] for d in data])
        labels = labels[labels >= 0]
        freq = np.bincount(labels, minlength=N_ACTIONS).astype(np.float32)
        w = np.sqrt(freq.sum() / np.maximum(freq, 1.0))
        class_w = torch.tensor(np.clip(w / np.median(w), 0.3, 3.0))
        loss = train_epochs(arm, data, opt, class_w, n_epochs=epochs_per_round,
                            rng=rng)
        val, vagree = eval_success(task, val_seeds, organs, arm)
        rec = dict(round=r, beta=beta, loss=loss, val_success=val,
                   val_probe_agree=vagree, n_eps=len(data))
        log.append(rec)
        print(f"[{task}/{arm_name}/s{seed}] r{r} beta={beta} loss={loss:.3f} "
              f"val={val:.3f}", flush=True)
        if val >= best:
            best = val
            torch.save({"arm": arm.state_dict(), "arm_name": arm_name,
                        "task": task, "seed": seed, "val_success": val,
                        "n_params": count_params(arm)}, out / "best.pt")
        (out / "metrics.jsonl").write_text(
            "\n".join(json.dumps(x) for x in log))
    return best


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--arm", required=True, choices=list(ARMS))
    ap.add_argument("--task", required=True)
    ap.add_argument("--seeds", type=int, nargs="+", default=[0])
    ap.add_argument("--rounds", type=int, default=6)
    ap.add_argument("--episodes", type=int, default=64)
    ap.add_argument("--out", default=str(CKPT_ORGANS.parent))
    ap.add_argument("--organ-dir", default=str(CKPT_ORGANS))
    ap.add_argument("--lr", type=float, default=3e-3)
    a = ap.parse_args()
    t0 = time.time()
    for s in a.seeds:
        run(a.task, a.arm, s, a.rounds, a.episodes, a.out, a.organ_dir, lr=a.lr)
    print(f"done in {time.time() - t0:.1f}s")


if __name__ == "__main__":
    main()
