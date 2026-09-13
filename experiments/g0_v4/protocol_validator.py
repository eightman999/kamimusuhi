"""G0-v4 protocol validator (spec §20).

Automated checks that the experiment cannot silently cheat:

  scope     train/val data contains ONLY train contexts and TRAIN_PAIRS
  eval      held-out datasets really are held out (ctx6/ctx8, OOD pairs,
            mid-episode switch to the held-out context)
  labels    no forbidden key (cause/ctx/seg/canonical/...) is accessed
            by the training-path modules; trainer sees exactly
            {obs, next_obs, actions} (meta.json audit)
  artifacts run dirs carry latest.pt/best.pt/config.yaml/meta.json/
            metrics.jsonl; device matches the request (no silent CPU
            fallback); OOM retries <= 1; no NaN losses logged

    python -m experiments.g0_v4.protocol_validator \
        --config experiments/g0_v4/configs/default.yaml \
        --run-dirs experiments/g0_v4/runs/cpc__seed0

Exit code 0 = PASS, 1 = FAIL. The report generator surfaces a FAIL
prominently.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from pathlib import Path

import numpy as np

from experiments.g0.data import train_val_datasets
from experiments.g0.env.dynamics import OOD_PAIRS, TRAIN_PAIRS
from experiments.g0.evaluate import _eval_datasets

from .config import load_config
from .train import TRAIN_KEYS

# eval-only PER-STEP metadata that must never feed the training loss.
# (env_seed / cause_names are per-dataset provenance recorded in
# meta.json, not per-step labels — they cannot leak content into a
# representation and are not scanned here; they are still stripped from
# the trainer's tensor dict by TRAIN_KEYS whitelisting.)
FORBIDDEN_KEYS = ("cause_a", "cause_b", "ctx", "ctx_id", "seg_id",
                  "x_a", "x_b", "canonical", "signal", "switch")

# training-path modules scanned for label access (eval/report modules
# legitimately read labels, so they are excluded)
TRAINING_PATH_FILES = ("models.py", "losses.py", "augment.py",
                       "train.py", "health.py", "config.py")

KEY_ACCESS = re.compile(
    r"""(?:\[\s*|\.get\(\s*)["'](""" + "|".join(FORBIDDEN_KEYS)
    + r""")["']""")


def check_scope(cfg, seed: int) -> dict:
    """Train/val: only train contexts, only TRAIN_PAIRS, same env_seed."""
    tr, va = train_val_datasets(cfg, seed)
    issues = []
    train_ctxs = set(range(cfg.env.n_train_contexts))
    for tag, ds in (("train", tr), ("val", va)):
        ctx_seen = set(np.unique(ds["ctx"]).tolist())
        bad_ctx = ctx_seen - train_ctxs
        if bad_ctx:
            issues.append(f"{tag}: held-out ctx leaked {bad_ctx}")
        pair_mask = ds["cause_b"] >= 0
        if pair_mask.any():
            pairs = {tuple(sorted(p)) for p in
                     zip(ds["cause_a"][pair_mask].ravel().tolist(),
                         ds["cause_b"][pair_mask].ravel().tolist())}
            bad = pairs - {tuple(sorted(p)) for p in TRAIN_PAIRS}
            if bad:
                issues.append(f"{tag}: OOD pair(s) leaked {bad}")
    if tr["env_seed"] != va["env_seed"]:
        issues.append("train/val env_seed differ (different worlds)")
    if np.allclose(tr["obs"][: va["obs"].shape[0]], va["obs"]):
        issues.append("train/val episodes identical")
    return {"name": "scope", "pass": not issues, "issues": issues}


def check_eval_isolation(cfg, seed: int) -> dict:
    """Eval datasets: held-out ctxs/pairs really held out; midctx switch."""
    dss = _eval_datasets(cfg, seed, seed)
    ood_id, dense_id = cfg.eval.ood.ood_ctx_id, \
        cfg.eval.ood.ood_dense_ctx_id
    issues = []
    if ood_id < cfg.env.n_train_contexts \
            or dense_id < cfg.env.n_train_contexts:
        issues.append("held-out ctx ids fall inside the train pool")
    if not (set(np.unique(dss["ood_ctx"]["ctx"])) == {ood_id}):
        issues.append("ood_ctx dataset mixes contexts")
    if not (set(np.unique(dss["dense_ctx"]["ctx"])) == {dense_id}):
        issues.append("dense_ctx dataset mixes contexts")
    mid = dss["midctx"]["ctx"]
    half = mid.shape[1] // 2
    if not (np.all(mid[:, : half - 1] == 0)
            and np.all(mid[:, half + 1:] == ood_id)):
        issues.append("midctx switch is not train-ctx0 -> held-out")
    ood_pairs = {tuple(sorted(p)) for p in OOD_PAIRS}
    for tag in ("combo", "combo_oodctx"):
        ds = dss[tag]
        pm = ds["cause_b"] >= 0
        if pm.any():
            seen = {tuple(sorted(p)) for p in
                    zip(ds["cause_a"][pm].ravel().tolist(),
                        ds["cause_b"][pm].ravel().tolist())}
            bad = seen - ood_pairs
            if bad:
                issues.append(f"{tag}: non-OOD pairs present {bad}")
    if not set(np.unique(dss["combo_oodctx"]["ctx"])) == {ood_id}:
        issues.append("combo_oodctx not rendered in held-out ctx")
    return {"name": "eval_isolation", "pass": not issues,
            "issues": issues}


def check_label_access(src_dir: Path) -> dict:
    """Static scan: training-path files must not index eval-only keys."""
    hits = []
    for fname in TRAINING_PATH_FILES:
        path = src_dir / fname
        if not path.exists():
            continue
        for i, line in enumerate(
                path.read_text().splitlines(), start=1):
            if KEY_ACCESS.search(line):
                hits.append(f"{fname}:{i}: {line.strip()[:100]}")
    # AST check on the augment module: it must not import env dynamics
    aug = src_dir / "augment.py"
    if aug.exists():
        tree = ast.parse(aug.read_text())
        for node in ast.walk(tree):
            if isinstance(node, (ast.Import, ast.ImportFrom)):
                mod = getattr(node, "module", "") or ""
                names = [a.name for a in node.names]
                if "dynamics" in mod or "dynamics" in names \
                        or "latent_cause_env" in mod:
                    hits.append(f"augment.py imports env internals: "
                                f"{mod or names}")
    return {"name": "label_access", "pass": not hits, "hits": hits}


def check_run(run_dir: Path, want_device: str | None = None) -> dict:
    issues = []
    for f in ("latest.pt", "best.pt", "config.yaml", "meta.json",
              "metrics.jsonl"):
        if not (run_dir / f).exists():
            issues.append(f"missing artifact {f}")
    meta_p = run_dir / "meta.json"
    if not meta_p.exists():
        return {"name": f"run:{run_dir.name}", "pass": False,
                "issues": issues}
    meta = json.loads(meta_p.read_text())
    if list(meta.get("data_keys", [])) != list(TRAIN_KEYS):
        issues.append(f"trainer saw keys {meta.get('data_keys')} "
                      f"!= {list(TRAIN_KEYS)}")
    if meta.get("oom_retries", 0) > 1:
        issues.append("more than one OOM retry")
    if meta.get("status") != "ok":
        issues.append(f"status={meta.get('status')}")
    if want_device and want_device.startswith("cuda"):
        if not str(meta.get("device", "")).startswith("cuda"):
            issues.append(f"ran on {meta.get('device')} — silent CPU "
                          f"fallback (requested {want_device})")
    mj = run_dir / "metrics.jsonl"
    if mj.exists():
        for i, line in enumerate(mj.read_text().splitlines()):
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                issues.append(f"metrics.jsonl:{i+1} unparsable")
                continue
            for k in ("train", "val"):
                v = rec.get(k)
                if v is not None and not np.isfinite(v):
                    issues.append(f"metrics.jsonl:{i+1} {k}={v} nonfinite")
    return {"name": f"run:{run_dir.name}", "pass": not issues,
            "issues": issues}


def validate(cfg_path, seed: int = 0, run_dirs=(),
             want_device: str | None = None) -> dict:
    cfg = load_config(cfg_path)
    checks = [check_scope(cfg, seed), check_eval_isolation(cfg, seed),
              check_label_access(Path(__file__).parent)]
    for rd in run_dirs:
        checks.append(check_run(Path(rd), want_device))
    ok = all(c["pass"] for c in checks)
    return {"pass": ok, "checks": checks, "config": str(cfg_path),
            "seed": seed}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(Path(__file__).parent /
                                            "configs/default.yaml"))
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--run-dirs", nargs="*", default=[])
    ap.add_argument("--device", default=None)
    ap.add_argument("--json", default=None)
    args = ap.parse_args()
    rep = validate(args.config, args.seed, args.run_dirs, args.device)
    txt = json.dumps(rep, indent=2, default=str)
    print(txt)
    if args.json:
        Path(args.json).write_text(txt)
    sys.exit(0 if rep["pass"] else 1)


if __name__ == "__main__":
    main()
