"""CX0 experiment matrix driver — trains every (task, arm, seed) cell with
BC -> PPO, then runs the intervention battery. Parallel workers.

Usage:
  python -m experiments.cx0.run_matrix --tasks ctx1 ctx3 --arms c1 c3 \
      --seeds 0 1 --workers 4 --out experiments/cx0/runs
  python -m experiments.cx0.run_matrix --smoke          # tiny sanity pass
"""

from __future__ import annotations

import argparse
import itertools
import multiprocessing as mp
import sys
import time
import traceback
from pathlib import Path

TASKS_ALL = ["ctx1", "ctx2", "ctx3", "ctx4", "ctx5"]
ARMS_ALL = ["c0", "c1", "c2", "c3"]
RUNS = Path(__file__).parent / "runs"


def worker(q, out, organ_dir, bc_rounds, bc_eps, ppo_iters, ppo_eps,
           eval_eps, verbose):
    import torch
    torch.set_num_threads(1)
    from .env.ctx_world import Oracle
    from .models.arms import build_arm, count_params
    from .organs.pretrain import build_organ_set
    from .runner import rollout
    from .train_bc import collect_round, train_epochs, eval_success
    from .train_ppo import collect_batch, ppo_update
    from .evaluate import run_eval
    import numpy as np
    import json

    organs = build_organ_set(organ_dir)
    val_seeds = [700001 + 1000 * k for k in range(8)]
    while True:
        item = q.get()
        if item is None:
            return
        task, arm_name, seed = item
        t0 = time.time()
        try:
            rng = np.random.default_rng(seed)
            torch.manual_seed(seed)
            arm = build_arm(arm_name)
            opt = torch.optim.Adam(arm.parameters(), lr=1.5e-3)
            oracle = Oracle(task)
            data = []
            best = -1.0
            run_dir = Path(out) / task / arm_name / f"s{seed}"
            run_dir.mkdir(parents=True, exist_ok=True)
            for r in range(bc_rounds):
                beta = max(0.2, 0.9 - 0.06 * r)
                data += collect_round(task, seed * 1000 + r, organs, arm,
                                      oracle, beta, bc_eps, rng)
                if len(data) > 480:
                    data = data[-480:]
                train_epochs(arm, data, opt, None, n_epochs=5, rng=rng)
                if r % 4 == 3 or r == bc_rounds - 1:
                    val, _ = eval_success(task, val_seeds, organs, arm)
                    if val >= best:
                        best = val
                        torch.save({"arm": arm.state_dict(),
                                    "arm_name": arm_name, "task": task,
                                    "seed": seed, "val_success": val,
                                    "n_params": count_params(arm),
                                    "stage": "bc"},
                                   run_dir / "best.pt")
            # PPO fine-tune
            opt = torch.optim.Adam(arm.parameters(), lr=8e-4)
            best_ppo = best
            for it in range(ppo_iters):
                import dataclasses
                from .env.ctx_world import TASKS
                spec = TASKS[task]
                if task == "ctx3" and it < ppo_iters // 2:
                    spec = dataclasses.replace(spec, resp_delay_range=(16, 40))
                eps = collect_batch(task, 300000 + seed * 1000 + it * 64,
                                    organs, arm, ppo_eps, rng, spec=spec)
                ppo_update(arm, opt, eps, rng=rng)
                if it % 10 == 9 or it == ppo_iters - 1:
                    val, _ = eval_success(task, val_seeds, organs, arm)
                    if val >= best_ppo:
                        best_ppo = val
                        torch.save({"arm": arm.state_dict(),
                                    "arm_name": arm_name, "task": task,
                                    "seed": seed, "val_success": val,
                                    "n_params": count_params(arm),
                                    "stage": "ppo"},
                                   run_dir / "ppo_best.pt")
            rows = run_eval(task, arm_name, [seed], eval_eps, out, organ_dir)
            res = Path(out) / task / arm_name
            res.mkdir(parents=True, exist_ok=True)
            with open(res / "results.jsonl", "a") as f:
                for rrow in rows:
                    f.write(json.dumps(rrow) + "\n")
            stat = ("OK", round(time.time() - t0))
        except Exception:
            stat = ("FAIL", traceback.format_exc(limit=3))
        print(f"[{task}/{arm_name}/s{seed}] {stat[0]} {stat[1]}",
              file=sys.stderr, flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--tasks", nargs="+", default=TASKS_ALL)
    ap.add_argument("--arms", nargs="+", default=ARMS_ALL)
    ap.add_argument("--seeds", type=int, nargs="+", default=[0, 1, 2, 3, 4])
    ap.add_argument("--workers", type=int, default=6)
    ap.add_argument("--bc-rounds", type=int, default=16)
    ap.add_argument("--bc-eps", type=int, default=48)
    ap.add_argument("--ppo-iters", type=int, default=30)
    ap.add_argument("--ppo-eps", type=int, default=32)
    ap.add_argument("--eval-eps", type=int, default=48)
    ap.add_argument("--out", default=str(RUNS))
    ap.add_argument("--organ-dir", default=str(RUNS / "organs"))
    ap.add_argument("--smoke", action="store_true")
    a = ap.parse_args()
    if a.smoke:
        a.tasks, a.arms, a.seeds = ["ctx3"], ["c1", "c3"], [0]
        a.bc_rounds, a.bc_eps, a.ppo_iters, a.ppo_eps, a.eval_eps = 4, 24, 5, 16, 16
    # pretrain organs once in the parent (workers share frozen ckpts)
    from .organs.pretrain import build_organ_set
    build_organ_set(a.organ_dir)
    cells = list(itertools.product(a.tasks, a.arms, a.seeds))
    print(f"{len(cells)} cells x {a.workers} workers", flush=True)
    q = mp.Queue()
    for c in cells:
        q.put(c)
    for _ in range(a.workers):
        q.put(None)
    procs = [mp.Process(target=worker, args=(q, a.out, a.organ_dir,
                                             a.bc_rounds, a.bc_eps,
                                             a.ppo_iters, a.ppo_eps,
                                             a.eval_eps, False))
             for _ in range(a.workers)]
    t0 = time.time()
    for p in procs:
        p.start()
    for p in procs:
        p.join()
    # workers append concurrently; dedup once it is safe to rewrite files
    from .evaluate import dedup_jsonl
    dropped = sum(dedup_jsonl(f) for f in Path(a.out).rglob("results.jsonl"))
    if dropped:
        print(f"dedup: {dropped} duplicate rows removed", flush=True)
    print(f"matrix done in {(time.time() - t0) / 60:.1f} min", flush=True)


if __name__ == "__main__":
    main()
