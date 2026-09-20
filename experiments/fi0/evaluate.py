"""Fi0 evaluation entry point.

    .venv/bin/python -m experiments.fi0.evaluate \
        --config experiments/fi0/configs/base.yaml \
        --model mock --results-dir experiments/fi0/results

Writes ``results/<run_id>/{raw.jsonl,manifest.json,analysis.json}`` and
prints a one-line verdict.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

from experiments.fi0.runners.clients import make_client
from experiments.fi0.runners.runner import run_evaluation
from experiments.fi0.sweep import load_config


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description="Fi0 prompt-stability sweep")
    ap.add_argument(
        "--config",
        default=str(Path(__file__).parent / "configs" / "base.yaml"),
    )
    ap.add_argument("--model", default=None, help="override config model")
    ap.add_argument("--reps", type=int, default=None)
    ap.add_argument(
        "--results-dir",
        default=str(Path(__file__).parent / "results"),
    )
    ap.add_argument("--run-id", default=None)
    args = ap.parse_args(argv)

    cfg = load_config(args.config)
    if args.model is not None or args.reps is not None:
        import dataclasses

        cfg = dataclasses.replace(
            cfg,
            model=args.model or cfg.model,
            reps=args.reps or cfg.reps,
        )
    client = make_client(cfg.model, seed=cfg.seed)
    out = run_evaluation(
        cfg, client, Path(args.results_dir), run_id=args.run_id
    )

    ov = out["analysis"]["overall"]
    print(f"run_dir: {out['run_dir']}")
    print(f"rows: {out['n_rows']}")
    print(f"mean_pairwise_agreement: {ov['mean_pairwise_agreement']:.4f}")
    print(f"mean_js_divergence:      {ov['mean_js_divergence']:.6f}")
    print(f"semantic_consistency:    {ov['mean_semantic_consistency']:.4f}")
    print(f"persona_consistency:     {ov['mean_persona_consistency']:.4f}")
    print(
        "canonical_collapses_variants: "
        f"{ov['canonical_collapses_variants']}"
    )
    print(f"VERDICT: {out['analysis']['verdict']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
