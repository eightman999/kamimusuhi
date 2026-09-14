"""CX0 evaluation battery — clean success, organ shuffles/lesions,
cortex-off, hidden resets, memory erase, population lesions, and
context-decode probes (C-G6).

Conditions (per task x arm x seed):
  clean | shuffle_{sensory,h0,s0,t0,r0} | cortex_off |
  reset_hidden@T | reset_organs@T | erase_memory@crisis |
  lesion_pop_{context,feedback,slow} (where the arm has them)

Donor-shuffle protocol: a donor episode (fixed eval seed +999) is rolled
under the same policy; its per-step field vector replaces the target's —
input distribution preserved, causal alignment destroyed (R0-C1 analog).

Outputs: results.jsonl rows + summary.json in the run dir; the analysis
step aggregates into reports/CX0_RESULTS.md.

Usage:
  python -m experiments.cx0.evaluate --task ctx1 --arm c3 --seeds 0 1 \
      --eps 64 --out experiments/cx0/runs
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
import torch

from .env.ctx_world import EP_LEN
from .models.arms import ARMS, build_arm, count_params
from .organs.base import DIMS, FIELDS
from .organs.pretrain import build_organ_set
from .runner import Intervention, rollout

EVAL_SEED0 = 900001
DONOR_SEED = 999999
RESULT_KEY = ("task", "arm", "seed", "condition")


def dedup_jsonl(path: Path) -> int:
    """Drop earlier duplicate rows (same task/arm/seed/condition), keeping
    the latest write. results.jsonl files are append-only, so rerunning a
    cell would otherwise silently double-count in report means."""
    if not path.exists():
        return 0
    rows = [json.loads(l) for l in path.read_text().splitlines() if l.strip()]
    seen, out = set(), []
    for r in reversed(rows):
        k = tuple(r.get(f) for f in RESULT_KEY)
        if k not in seen:
            seen.add(k)
            out.append(r)
    out.reverse()
    if len(out) != len(rows):
        path.write_text("".join(json.dumps(r) + "\n" for r in out))
    return len(rows) - len(out)


# ----------------------------------------------------------------------
def load_arm(path: Path, device="cpu"):
    ck = torch.load(path, weights_only=True)
    arm = build_arm(ck["arm_name"])
    arm.load_state_dict(ck["arm"])
    arm.eval()
    return arm, ck


def crisis_step(task: str, env_probe_seed: int = 0) -> int:
    """Representative intervention time: just before typical crisis/go."""
    from .env.ctx_world import TASKS
    s = TASKS[task]
    if s.go_window:
        return s.go_window[0]
    if s.crisis_delay_range:
        return s.crisis_delay_range[0] + s.cue_at
    return EP_LEN // 2


def ridge_probe(X: np.ndarray, y: np.ndarray, n_classes: int,
                frac_train: float = 0.7, seed: int = 0) -> float:
    """Closed-form ridge readout: one-hot targets, argmax accuracy.
    Used for C-G6 (context decodable from a population)."""
    rng = np.random.default_rng(seed)
    idx = rng.permutation(len(X))
    ntr = int(len(X) * frac_train)
    Xtr, Xte = X[idx[:ntr]], X[idx[ntr:]]
    ytr, yte = y[idx[:ntr]], y[idx[ntr:]]
    Ytr = np.eye(n_classes)[np.clip(ytr, 0, n_classes - 1)]
    Xm = np.concatenate([Xtr, np.ones((len(Xtr), 1))], 1)
    W = np.linalg.solve(Xm.T @ Xm + 1e-3 * np.eye(Xm.shape[1]), Xm.T @ Ytr)
    Xe = np.concatenate([Xte, np.ones((len(Xte), 1))], 1)
    pred = np.argmax(Xe @ W, 1)
    return float((pred == yte).mean())


def eval_condition(task, arm, organs, itv, seeds, donor_seq=None,
                   record_pops=False):
    succ, dead, agree, pops_all, labels_all = [], 0, [], {}, []
    for i, s in enumerate(seeds):
        tr = rollout(task, seed=s, organs=organs, arm=arm, itv=itv,
                     donor=(donor_seq[i % len(donor_seq)] if donor_seq else None),
                     record_pops=record_pops)
        succ.append(float(tr.success))
        dead += int(tr.dead)
        if tr.probe_agree is not None:
            agree.append(tr.probe_agree)
        if record_pops:
            for k, v in tr.pops.items():
                pops_all.setdefault(k, []).append(np.stack(v))
            labels_all.append(np.array(tr.ctx_labels))
    out = dict(success=float(np.mean(succ)), dead=dead,
               probe_agree=(float(np.mean(agree)) if agree else None))
    if record_pops:
        out["_pops"] = pops_all
        out["_labels"] = np.concatenate(labels_all)
    return out


def run_eval(task, arm_name, seeds, eps, out_dir, organ_dir, device="cpu"):
    from .env.ctx_world import TASKS
    organs = build_organ_set(organ_dir, device=device)
    rows = []
    for seed in seeds:
        run_dir = Path(out_dir) / task / arm_name / f"s{seed}"
        # PPO fine-tune supersedes the BC checkpoint when present
        ckpt = run_dir / "ppo_best.pt" if (run_dir / "ppo_best.pt").exists() \
            else run_dir / "best.pt"
        if not ckpt.exists():
            print(f"missing {run_dir}/(ppo_)best.pt — skipped")
            continue
        arm, meta = load_arm(ckpt, device)
        eval_seeds = [EVAL_SEED0 + 37 * i for i in range(eps)]
        t_int = crisis_step(task)

        # donor episodes for field shuffles (policy-driven, same arm)
        donor_seq = []
        for i in range(min(4, eps)):
            tr_d = rollout(task, seed=DONOR_SEED + 31 * i, organs=organs,
                           arm=arm, record_pops=False)
            donor_seq.append(tr_d.bundles)   # list of (BUNDLE_DIM,) arrays
        donor_fields = {
            f: [[np.asarray(b)[_slice(f)] for b in d] for d in donor_seq]
            for f in FIELDS
        }

        conds = {"clean": Intervention()}
        for f in ("sensory", "h0", "s0", "t0", "r0"):
            conds[f"shuffle_{f}"] = Intervention(shuffle=f)
        conds["cortex_off"] = Intervention(cortex_off=True)
        conds["reset_hidden"] = Intervention(reset_hidden_at=t_int)
        conds["reset_organs"] = Intervention(reset_organs_at=t_int)
        conds["erase_memory"] = Intervention(erase_memory_at=t_int)
        for p in ("context", "feedback", "slow"):
            conds[f"lesion_{p}"] = Intervention(lesion_pop=p)
        # field lesions: zero a whole organ field — the causality test for
        # fields whose donor-shuffle is a near-no-op (stereotyped signals)
        for f in ("h0", "s0", "t0", "r0"):
            conds[f"lesion_{f}"] = Intervention(lesion=f)

        for cname, itv in conds.items():
            donor = donor_fields.get(itv.shuffle) if itv.shuffle else None
            rec_pops = (cname == "clean")
            r = eval_condition(task, arm, organs, itv, eval_seeds,
                               donor_seq=donor, record_pops=rec_pops)
            row = dict(task=task, arm=arm_name, seed=seed, condition=cname,
                       success=r["success"], dead=r["dead"],
                       probe_agree=r["probe_agree"],
                       n_params=meta.get("n_params"))
            if rec_pops:
                labels = r["_labels"]
                ncls = int(labels.max()) + 1 if labels.size else 1
                for pop, arrs in r["_pops"].items():
                    X = np.concatenate(arrs)
                    row[f"probe_{pop}"] = ridge_probe(X, labels, ncls)
            rows.append(row)
            print(f"[{task}/{arm_name}/s{seed}] {cname}: "
                  f"success={r['success']:.3f}", flush=True)
    return rows


def _slice(field: str):
    off = 0
    for k, v in DIMS.items():
        if k == field:
            return slice(off, off + v)
        off += v
    raise KeyError(field)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--task", required=True)
    ap.add_argument("--arm", required=True, choices=list(ARMS))
    ap.add_argument("--seeds", type=int, nargs="+", default=[0])
    ap.add_argument("--eps", type=int, default=64)
    ap.add_argument("--out", default=str(Path(__file__).parent / "runs"))
    ap.add_argument("--organ-dir", default=str(Path(__file__).parent / "runs" / "organs"))
    a = ap.parse_args()
    rows = run_eval(a.task, a.arm, a.seeds, a.eps, a.out, a.organ_dir)
    res = Path(a.out) / a.task / a.arm
    res.mkdir(parents=True, exist_ok=True)
    with open(res / "results.jsonl", "a") as f:
        for r in rows:
            f.write(json.dumps(r) + "\n")
    dedup_jsonl(res / "results.jsonl")


if __name__ == "__main__":
    main()
