"""C0 evaluation orchestrator.

Headline results run through the REAL process boundary: for every
(episode batch, condition, causal-variant) cell, evaluate.py invokes
run_phase_a.py and run_phase_b.py as subprocesses; the only thing carried
between them is the artifact file on disk.

Cell kinds:
    conditions  cold | hidden | memory | compressed@B | full
    causal      wrong_state | mask{25,50,75} | shuffle |
                gap{20,40,80} | double (two interruptions)
    ood         long_ep | early_stop | late_stop | more_bridge | highnoise
    swap        (stretch) artifact of ckpt A restored into ckpt B

Plus an uninterrupted in-process reference (for degradation / latent
similarity) and the storeall heuristic as a training-free subject.

    python -m experiments.c0.evaluate --checkpoint artifacts/runs/g64_s0/best.pt \
        --episodes 48 --workdir artifacts/eval/g64_s0
"""
from __future__ import annotations

import argparse
import concurrent.futures as cf
import json
import os
import subprocess
import sys
import time
from dataclasses import asdict
from pathlib import Path

import numpy as np
import torch
import yaml

from .agents.policies import MODEL_SPECS, build_policy
from .agents.runner import Runner
from .config_util import git_commit, load_env_cfg, load_policy
from .env.c0_env import C0Env, C0Config
from .persistence.artifact import read_artifact, write_artifact
from .persistence.codec import (apply_condition, decode_payload,
                                encode_lossless, mask_state, shuffle_state,
                                state_spec_from)
from .persistence.interrupt import run_interrupted

ROOT = Path(__file__).resolve().parents[2]
BUDGETS = [16, 64, 256, 1024, 4096]
MASKS = [0.25, 0.5, 0.75]
GAPS = [20, 40, 80]
DONOR_OFFSET = 50_000


def spec_for(model_name: str, env_cfg: C0Config) -> list[tuple]:
    fields = []
    if MODEL_SPECS[model_name]["recurrent"]:
        fields.append("hidden")
    if MODEL_SPECS[model_name]["memory"]:
        fields += ["mem_payloads", "mem_keys", "mem_occupied",
                   "mem_insert", "mem_clock"]
    return state_spec_from(fields, env_cfg,
                           MODEL_SPECS[model_name]["hidden"])


def _phase_cmd(script: str, args) -> list[str]:
    return [sys.executable, "-m", f"experiments.c0.{script}"] + args


def _run(cmd: list[str], log=None) -> None:
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)}\n{r.stdout}\n{r.stderr}")


def run_cell(ckpt: str | None, heuristic: str | None, cfg_path: str,
             over: list[str], condition: str, budget: int | None,
             n_eps: int, ep_base: int, workdir: Path, tag: str,
             causal: str | None = None, mask_frac: float | None = None,
             model_override: str | None = None,
             env_cfg: C0Config | None = None, device="cpu") -> dict:
    """One cell = phase A (subprocess) -> optional artifact perturbation ->
    phase B leg(s) (subprocess). Returns merged per-query records."""
    workdir.mkdir(parents=True, exist_ok=True)
    src = (["--checkpoint", ckpt] if ckpt else ["--heuristic", heuristic])
    ov_args = ["--model-override", model_override] if model_override else []
    set_args = ["--set"] + over if over else []
    n_stops = env_cfg.n_stops if env_cfg is not None else 1

    # -- leg A ---------------------------------------------------------
    art_a = workdir / f"{tag}_artA.json"
    seg_a = workdir / f"{tag}_segA.json"
    _run(_phase_cmd("run_phase_a", src + [
        "--config", cfg_path, "--episodes", str(n_eps),
        "--ep-seed-base", str(ep_base), "--condition", condition,
        "--artifact-out", str(art_a), "--metrics-out", str(seg_a)]
        + (["--budget", str(budget)] if budget else []) + set_args))

    # -- causal artifact perturbation -----------------------------------
    art_in = art_a
    if causal == "wrong_state":
        art_d = workdir / f"{tag}_artD.json"
        seg_d = workdir / f"{tag}_segD.json"
        _run(_phase_cmd("run_phase_a", src + [
            "--config", cfg_path, "--episodes", str(n_eps),
            "--ep-seed-base", str(ep_base + DONOR_OFFSET),
            "--condition", condition,
            "--artifact-out", str(art_d), "--metrics-out", str(seg_d)]
            + (["--budget", str(budget)] if budget else []) + set_args))
        art_in = art_d
    elif causal in ("mask", "shuffle"):
        # perturb the *decoded* state, re-encode losslessly (condition full)
        doc, payloads = read_artifact(art_a)
        assert env_cfg is not None and ckpt
        full_spec = spec_for(doc["agent"], env_cfg)
        rng = np.random.default_rng(ep_base + 777)
        pert = []
        for p in payloads:
            st = decode_payload(p, condition, full_spec)
            # restrict the perturbation spec to fields actually restored
            spec = [e for e in full_spec if e[0] in st]
            if causal == "mask":
                st = mask_state(st, mask_frac or 0.5, rng, spec)
            else:
                st = shuffle_state(st, rng, spec)
            pert.append(encode_lossless(st))
        art_p = workdir / f"{tag}_artP.json"
        write_artifact(art_p, agent=doc["agent"], condition="full",
                       payloads=pert)
        art_in = art_p
    # -- legs B ----------------------------------------------------------
    segs = [seg_a]
    prev_art = art_in
    for k in range(n_stops):
        seg_b = workdir / f"{tag}_segB{k}.json"
        more = k + 1 < n_stops
        art_next = workdir / f"{tag}_artB{k}.json" if more else None
        cmd = _phase_cmd("run_phase_b", src + ov_args + [
            "--config", cfg_path, "--episodes", str(n_eps),
            "--ep-seed-base", str(ep_base), "--artifact-in", str(prev_art),
            "--stop-index", str(k), "--metrics-out", str(seg_b)]
            + (["--artifact-out", str(art_next)] if more else [])
            + set_args)
        _run(cmd)
        segs.append(seg_b)
        prev_art = art_next

    # -- merge -----------------------------------------------------------
    recs = {}
    ep_stats = {}
    hidden_resume = {}
    for sg in segs:
        m = json.loads(sg.read_text())
        for ep in m["episodes"]:
            recs.setdefault(ep["i"], []).extend(ep.get("queries", []))
            if "ep_stats" in ep:
                ep_stats[ep["i"]] = ep["ep_stats"]
            if ep.get("hidden_resume") is not None:
                hidden_resume[ep["i"]] = ep["hidden_resume"]
    rows = []
    for i, qs in recs.items():
        for q in qs:
            rows.append({"ep": i, **q})
    return {"tag": tag, "condition": condition, "budget": budget,
            "causal": causal, "mask_frac": mask_frac, "over": over,
            "n_episodes": n_eps, "ep_seed_base": ep_base,
            "rows": rows, "ep_stats": ep_stats,
            "hidden_resume": hidden_resume}


def doc2model(doc: dict) -> str:
    return doc["agent"]


def inprocess_reference(ckpt: str, env_cfg: C0Config, n_eps: int,
                        ep_base: int, device="cpu") -> dict:
    """Uninterrupted + full-restore latent reference (supplementary)."""
    policy, ck = load_policy(ckpt, env_cfg, device)
    spec = spec_for(ck["model_name"], env_cfg)
    envs = [C0Env(env_cfg) for _ in range(n_eps)]
    for i, e in enumerate(envs):
        e.reset(seed=ep_base + i)
    runner = Runner(policy, ck["model_name"], env_cfg, device)
    ref = run_interrupted(runner, envs, spec, uninterrupted=True,
                          record_latents=True)
    return {"records": ref["records"], "stats": ref["stats"],
            "latents": ref["latents"]}


def inprocess_condition(ckpt: str, env_cfg: C0Config, condition: str,
                        budget: int | None, n_eps: int, ep_base: int,
                        device="cpu") -> dict:
    """Fast in-process variant (sanity check / tests)."""
    policy, ck = load_policy(ckpt, env_cfg, device)
    spec = spec_for(ck["model_name"], env_cfg)
    envs = [C0Env(env_cfg) for _ in range(n_eps)]
    for i, e in enumerate(envs):
        e.reset(seed=ep_base + i)
    runner = Runner(policy, ck["model_name"], env_cfg, device)
    res = run_interrupted(runner, envs, spec, condition=condition,
                          budget=budget, record_latents=True)
    return res


def latent_similarity(ckpt: str, env_cfg: C0Config, condition: str,
                      budget: int | None, n_eps: int, ep_base: int,
                      device="cpu") -> dict:
    """Cosine between restored-run and uninterrupted-run hidden states."""
    policy, ck = load_policy(ckpt, env_cfg, device)
    if getattr(policy, "hidden", None) is None:
        return {}
    spec = spec_for(ck["model_name"], env_cfg)
    envs = [C0Env(env_cfg) for _ in range(n_eps)]
    for i, e in enumerate(envs):
        e.reset(seed=ep_base + i)
    r1 = Runner(policy, ck["model_name"], env_cfg, device)
    ref = run_interrupted(r1, envs, spec, uninterrupted=True,
                          record_latents=True)
    envs2 = [C0Env(env_cfg) for _ in range(n_eps)]
    for i, e in enumerate(envs2):
        e.reset(seed=ep_base + i)
    r2 = Runner(policy, ck["model_name"], env_cfg, device)
    res = run_interrupted(r2, envs2, spec, condition=condition,
                          budget=budget, record_latents=True)
    # per-step cosine over the post-resume portion
    stop0 = env_cfg.stops[0][0]
    L = min(len(ref["latents"]), len(res["latents"]))
    def cos_at(t):
        a, b = ref["latents"][t], res["latents"][t]
        num = (a * b).sum(1)
        den = np.linalg.norm(a, axis=1) * np.linalg.norm(b, axis=1) + 1e-9
        return float(np.mean(num / den))
    probes = {"resume+1": min(stop0 + 1, L - 1),
              "resume+8": min(stop0 + 8, L - 1),
              "resume+32": min(stop0 + 32, L - 1),
              "end": L - 1}
    return {k: cos_at(t) for k, t in probes.items()}


def summarize_rows(rows: list[dict]) -> dict:
    out = {}
    for cls in ("pre", "post", "ctrl"):
        sel = [r["correct"] for r in rows if r["cls"] == cls]
        out[f"acc_{cls}"] = float(np.mean(sel)) if sel else float("nan")
        out[f"n_{cls}"] = len(sel)
    allc = [r["correct"] for r in rows]
    out["acc_all"] = float(np.mean(allc)) if allc else float("nan")
    out["answered"] = float(np.mean([r["answered"] for r in rows])) \
        if rows else float("nan")
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    src = ap.add_mutually_exclusive_group(required=True)
    src.add_argument("--checkpoint")
    src.add_argument("--heuristic", choices=["random", "oracle", "storeall"])
    ap.add_argument("--config", default="experiments/c0/configs/default.yaml")
    ap.add_argument("--episodes", type=int, default=48)
    ap.add_argument("--ep-seed-base", type=int, default=900000)
    ap.add_argument("--workdir", default=None)
    ap.add_argument("--cells", nargs="*", default=None,
                    help="subset e.g. cold full wrong_state")
    ap.add_argument("--parallel", type=int, default=8)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--swap-ckpt", default=None,
                    help="checkpoint B for the model-swap stretch test")
    ap.add_argument("--inprocess", action="store_true",
                    help="fast path: skip the subprocess boundary (tests)")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    env_cfg, env_d = load_env_cfg(args.config)
    model_name = None
    if args.checkpoint:
        ck = torch.load(args.checkpoint, map_location="cpu",
                        weights_only=False)
        model_name = ck["model_name"]
        subject = f"{model_name}_{Path(args.checkpoint).parent.name}"
    else:
        subject = f"heur_{args.heuristic}"
    workdir = Path(args.workdir or
                   f"experiments/c0/artifacts/eval/{subject}")
    workdir.mkdir(parents=True, exist_ok=True)
    t0 = time.time()

    has_mem = (MODEL_SPECS[model_name]["memory"] if model_name
               else args.heuristic == "storeall")
    conds = [("cold", None), ("hidden", None), ("full", None)]
    if has_mem:
        conds.insert(2, ("memory", None))
    conds += [("compressed", b) for b in BUDGETS]

    jobs = []   # (label, kwargs)

    def add(label, **kw):
        if args.cells and label.split(":")[0] not in args.cells \
                and label not in args.cells:
            return
        jobs.append((label, kw))

    for cond, bud in conds:
        add(f"cond:{cond}:{bud}", over=[], condition=cond, budget=bud)
    # causal battery on full + the model's signature condition
    # (learned checkpoints only -- heuristics run condition cells only)
    if args.checkpoint:
        sig = "memory" if has_mem else "hidden"
        for cond in sorted({"full", sig}):
            add(f"causal:wrong_state:{cond}", over=[], condition=cond,
                causal="wrong_state")
            add(f"causal:shuffle:{cond}", over=[], condition=cond,
                causal="shuffle")
            for mf in MASKS:
                add(f"causal:mask{int(mf*100)}:{cond}", over=[],
                    condition=cond, causal="mask", mask_frac=mf)
        for g in GAPS:
            add(f"causal:gap{g}:full", over=[f"stops=80:{g}"],
                condition="full")
        add("causal:double:full",
            over=["stops=50:0,100:0", "n_mid=1"], condition="full")
    # OOD env variants on full restore
    oods = {
        "long_ep": ["episode_len=220", "stops=100:0", "post_q_lo=24"],
        "early_stop": ["stops=40:0"],
        "late_stop": ["stops=120:0"],
        "more_bridge": ["n_bridge=3"],
        "highnoise": ["noise_rate=0.7", "num_distractors=14"],
    }
    for name, ov in oods.items():
        add(f"ood:{name}:full", over=ov, condition="full")
    # stretch: model swap
    if args.swap_ckpt:
        add("stretch:swap:full", over=[], condition="full",
            model_override=args.swap_ckpt)

    env_cache = {}

    def env_for(over):
        key = tuple(over)
        if key not in env_cache:
            env_cache[key] = load_env_cfg(args.config, over)[0]
        return env_cache[key]

    results = {}
    if args.inprocess and args.checkpoint:
        for label, kw in jobs:
            ec = env_for(kw["over"])
            res = inprocess_condition(
                args.checkpoint, ec, kw["condition"], kw.get("budget"),
                args.episodes, args.ep_seed_base, args.device)
            rows = [{"ep": i, **q} for i, qs in
                    enumerate(res["records"]) for q in qs]
            results[label] = {"rows": rows,
                              "summary": summarize_rows(rows)}
            print(f"[{label}] {results[label]['summary']}", flush=True)
    else:
        def one(job):
            label, kw = job
            ec = env_for(kw["over"])
            cell = run_cell(args.checkpoint, args.heuristic, args.config,
                            kw["over"], kw["condition"], kw.get("budget"),
                            args.episodes, args.ep_seed_base, workdir,
                            tag=label.replace(":", "-"),
                            causal=kw.get("causal"),
                            mask_frac=kw.get("mask_frac"),
                            model_override=kw.get("model_override"),
                            env_cfg=ec, device=args.device)
            return label, cell
        with cf.ThreadPoolExecutor(args.parallel) as ex:
            for label, cell in ex.map(one, jobs):
                cell["summary"] = summarize_rows(cell["rows"])
                results[label] = cell
                print(f"[{label}] {cell['summary']}", flush=True)

    # uninterrupted reference + latent similarity (in-process, cheap)
    ref = None
    if args.checkpoint:
        ref = inprocess_reference(args.checkpoint, env_cfg, args.episodes,
                                  args.ep_seed_base, args.device)
        ref_rows = [{"ep": i, **q} for i, qs in enumerate(ref["records"])
                    for q in qs]
        results["ref:uninterrupted"] = {"rows": ref_rows,
                                        "summary": summarize_rows(ref_rows)}
        for cond, bud in conds:
            results.setdefault("latent", {})
            sim = latent_similarity(args.checkpoint, env_cfg, cond, bud,
                                    min(16, args.episodes),
                                    args.ep_seed_base, args.device)
            results[f"latent:{cond}:{bud}"] = {"summary": sim}

    out = {"subject": subject, "checkpoint": args.checkpoint,
           "episodes": args.episodes, "ep_seed_base": args.ep_seed_base,
           "env": env_d, "git": git_commit(),
           "wall_sec": round(time.time() - t0, 1),
           "results": results}
    outp = Path(args.out or workdir / "eval.json")
    outp.write_text(json.dumps(out, indent=2))
    print(f"wrote {outp}")


if __name__ == "__main__":
    main()
