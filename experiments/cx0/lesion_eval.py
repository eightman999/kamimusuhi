"""Eval-only pass: run field-lesion conditions over saved checkpoints and
append rows to each arm's results.jsonl. For fields whose donor-shuffle is
a near-no-op (stereotyped signals like h0 internals), zeroing the field is
the real causality test.

Usage: python -m experiments.cx0.lesion_eval --runs experiments/cx0/runs_v2
"""

from __future__ import annotations

import argparse
import json
import multiprocessing as mp
import sys
from pathlib import Path


def work(q, organ_dir):
    import torch
    torch.set_num_threads(1)
    from .organs.pretrain import build_organ_set
    from .evaluate import load_arm, eval_condition, EVAL_SEED0
    from .runner import Intervention
    organs = build_organ_set(organ_dir)
    eval_seeds = [EVAL_SEED0 + 37 * i for i in range(48)]
    while True:
        it = q.get()
        if it is None:
            return
        ckpt, task, arm_name, seed, out_file = it
        try:
            arm, meta = load_arm(Path(ckpt))
            for f in ('h0', 's0', 't0', 'r0'):
                r = eval_condition(task, arm, organs,
                                   Intervention(lesion=f), eval_seeds)
                row = dict(task=task, arm=arm_name, seed=seed,
                           condition=f'lesion_{f}', success=r['success'],
                           dead=r['dead'], probe_agree=r['probe_agree'],
                           n_params=meta.get('n_params'))
                with open(out_file, 'a') as fh:
                    fh.write(json.dumps(row) + '\n')
            print(f'{task}/{arm_name}/s{seed} lesions done',
                  file=sys.stderr, flush=True)
        except Exception as e:
            print(f'{task}/{arm_name}/s{seed} FAIL {e}',
                  file=sys.stderr, flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--runs', required=True)
    ap.add_argument('--organ-dir', default=str(Path(__file__).parent
                                               / 'runs' / 'organs'))
    ap.add_argument('--workers', type=int, default=6)
    ap.add_argument('--only-arm', default=None)
    a = ap.parse_args()
    runs = Path(a.runs)
    q = mp.Queue()
    cells = list(runs.rglob('ppo_best.pt'))
    cells += [c for c in runs.rglob('best.pt')
              if not (c.parent / 'ppo_best.pt').exists()]
    for ck in cells:
        task = ck.parent.parent.parent.name
        arm_name = ck.parent.parent.name
        seed = int(ck.parent.name[1:])
        if a.only_arm and arm_name != a.only_arm:
            continue
        out = ck.parent.parent / 'results.jsonl'
        q.put((str(ck), task, arm_name, seed, str(out)))
    for _ in range(a.workers):
        q.put(None)
    procs = [mp.Process(target=work, args=(q, a.organ_dir))
             for _ in range(a.workers)]
    for p in procs:
        p.start()
    for p in procs:
        p.join()
    # workers append concurrently; dedup once it is safe to rewrite files
    from .evaluate import dedup_jsonl
    dropped = sum(dedup_jsonl(f) for f in runs.rglob('results.jsonl'))
    if dropped:
        print(f'dedup: {dropped} duplicate rows removed')
    print(f'lesion eval done over {len(cells)} checkpoints')


if __name__ == '__main__':
    main()
