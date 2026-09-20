"""Ad-hoc pilot/stage2 inspection: merge sweep JSONs, print trained vs
untrained transfer metrics and paired diffs. Not part of the protocol —
`report.py` is the canonical report generator; this is a console view.

    python -m experiments.g0_v4.summarize_runs reports/a.json,reports/b.json
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

import numpy as np

TRANSFER = (
    ("midctx", ("causal", "midctx_acc"), ("causal", "midctx_acc_null")),
    ("match", ("matching", "match_ood_ctx"),
     ("matching", "match_ood_ctx_null")),
    ("dsegOOD", ("probes", "dynseg_acc_ood_ctx"), None),
    ("acc_in", ("probes", "acc_in"), None),
    ("loco", ("probes", "acc_loco"), None),
    ("combo", ("ood", "combo_oodctx_auc"), None),
    ("sel", ("intervention", "selectivity"), None),
)


def _get(d, path):
    for k in path:
        if not isinstance(d, dict) or k not in d:
            return None
        d = d[k]
    return d


def main(paths):
    per = {}
    for p in paths:
        s = json.loads(Path(p).read_text())
        for rep, bs in (s.get("per_run") or {}).items():
            per.setdefault(rep, {}).update(bs)
    reps = sorted(per)
    trained = [r for r in reps if not r.endswith("_untrained")
               and not r.startswith(("raw", "pca", "dynfeat"))]
    for m in trained:
        u = m + "_untrained"
        for tag in (m, u):
            if tag not in per:
                continue
            cells = []
            seeds = sorted(per[tag], key=int)
            for label, path, nullp in TRANSFER:
                vs = [_get(per[tag][s], path) for s in seeds]
                vs = [v for v in vs
                      if isinstance(v, (int, float))
                      and not (isinstance(v, float) and np.isnan(v))]
                if not vs:
                    cells.append(f"{label}=-")
                    continue
                txt = f"{label}={np.mean(vs):.3f}"
                if nullp:
                    nv = [_get(per[tag][s], nullp) for s in seeds]
                    nv = [v for v in nv if isinstance(v, (int, float))]
                    txt += f"/{np.mean(nv):.3f}n"
                cells.append(txt)
            mark = "tr " if tag == m else "UN "
            print(f"{mark}{tag:40s} " + " ".join(cells))
        if u not in per:
            continue
        seeds = sorted(set(per[m]) & set(per[u]), key=int)
        out = []
        for label, path, _ in TRANSFER:
            ds = []
            for s in seeds:
                a, b = _get(per[m][s], path), _get(per[u][s], path)
                if isinstance(a, (int, float)) \
                        and isinstance(b, (int, float)) \
                        and not np.isnan(a) and not np.isnan(b):
                    ds.append(a - b)
            if ds:
                out.append(f"{label} {np.mean(ds):+.3f} "
                           f"({sum(d > 0 for d in ds)}/{len(ds)})")
        print("   diff " + " | ".join(out) + "\n")


if __name__ == "__main__":
    paths = sys.argv[1:] or ["experiments/g0_v4/reports/"
                             "sweep_summary.json"]
    main([p for arg in paths for p in arg.split(",")])
