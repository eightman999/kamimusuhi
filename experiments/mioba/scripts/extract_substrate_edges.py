"""Extract a renderable edge sample of the shared FBA0 substrate for the
Observatory's schematic region view.

The graph is regenerated through the same code path the torch backend
uses (``get_backend`` -> ``initialize`` -> ``W_base``), so the exported
pairs are a real subset of the run's connectivity — RECORDED data, not
an illustration. Positions are not exported: the GUI places neurons by
its own synthetic-region-v0 anchors and draws these edges between them.

Usage:
    python -m experiments.mioba.scripts.extract_substrate_edges \
        --neurons 139000 --edges 14000000 --base-seed 20260911 \
        --sample 24000 --out experiments/mioba/gui/static/substrate_edges.json
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path

import torch

from ..fba.registry import get_backend
from ..fba.torch_backend import SYNTHETIC_REGIONS


def region_of(index: int, n_base: int) -> int:
    per = n_base / len(SYNTHETIC_REGIONS)
    return min(len(SYNTHETIC_REGIONS) - 1, int(index // per))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--neurons", type=int, required=True)
    ap.add_argument("--edges", type=int, required=True)
    ap.add_argument("--base-seed", type=int, required=True)
    ap.add_argument("--connectivity", type=float, default=0.01)
    ap.add_argument("--sample", type=int, default=24000)
    ap.add_argument("--sample-seed", type=int, default=7)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    backend = get_backend("torch", synthetic=True,
                          synthetic_neurons=args.neurons,
                          synthetic_edges=args.edges,
                          base_seed=args.base_seed,
                          connectivity=args.connectivity)
    backend.initialize({"substrates": [{"substrate_id": "fba0"}]},
                       batch_size=1, seed=0, device="cpu",
                       replicate_seeds=[0])
    n = backend.n_base
    coo = backend.W_base.to_coo(size=(n, n)).coalesce()
    post, pre = coo.indices()[0], coo.indices()[1]
    nnz = post.numel()

    gen = torch.Generator().manual_seed(args.sample_seed)
    pick = torch.randperm(nnz, generator=gen)[: args.sample]
    pairs = torch.stack([pre[pick], post[pick]], dim=1).tolist()

    r_post = torch.div(post, max(1, n // len(SYNTHETIC_REGIONS)),
                       rounding_mode="floor").clamp_(0, len(SYNTHETIC_REGIONS) - 1)
    r_pre = torch.div(pre, max(1, n // len(SYNTHETIC_REGIONS)),
                      rounding_mode="floor").clamp_(0, len(SYNTHETIC_REGIONS) - 1)
    matrix = [[0] * len(SYNTHETIC_REGIONS) for _ in SYNTHETIC_REGIONS]
    for a, b in zip(r_pre.tolist(), r_post.tolist()):
        matrix[a][b] += 1

    doc = {
        "kind": "DERIVED",
        "substrate": {"synthetic": True, "n_base": n,
                      "requested_edges": args.edges, "nnz": int(nnz),
                      "base_seed": args.base_seed,
                      "connectivity": args.connectivity,
                      "region_mode": "synthetic-region-v0"},
        "regions": list(SYNTHETIC_REGIONS),
        "region_pair_edges": matrix,
        "edge_sample": {"seed": args.sample_seed, "count": len(pairs),
                        "pairs": pairs},
    }
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(doc, separators=(",", ":")))
    print(f"wrote {out}: nnz={nnz} sampled={len(pairs)} "
          f"{out.stat().st_size/1e6:.2f} MB")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
