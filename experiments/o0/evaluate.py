"""O0 evaluation engine.

Runs a model or a heuristic over freshly generated episodes and returns
metrics.  Supports hidden-state interventions (the causal tests):

    intervention = {"type": "none"}
    intervention = {"type": "hidden_reset", "at": "mid_occl"}
    intervention = {"type": "hidden_noise", "at": "mid_occl", "sigma": 1.0}

`at` selects per-episode target steps: "mid_occl" (midpoint of the first
occlusion bout), "occl_start" (first hidden step), "mid_vis" (midpoint
of the first *visible* phase — a control: the model can re-read position
from the observation and should recover).

OOD evaluation uses EnvParams overrides (PRESETS below).

CLI:
    python -m experiments.o0.evaluate --config experiments/o0/configs/default.yaml \
        --checkpoint artifacts/runs/gru64_s0/final.pt --episodes 512 \
        --preset id --out reports/eval_gru64_id.json
"""

from __future__ import annotations

import argparse
import dataclasses
import json
import time
from pathlib import Path
from typing import Dict, Optional

import numpy as np

from .analysis import metrics as metrics_mod
from .config import Config, load_config
from .env import dynamics as dyn

# Named OOD / causal condition presets: EnvParams overrides.
PRESETS: Dict[str, Dict] = {
    "id": {},
    # longer occlusion than trained (train range 4-16)
    "occ24": {"occ_lo": 24, "occ_hi": 24, "v_lo": 0.05, "v_hi": 0.07,
              "vis_lo": 2, "vis_hi": 4, "spawn_frac_hi": 0.10},
    "occ32": {"occ_lo": 32, "occ_hi": 32, "v_lo": 0.05, "v_hi": 0.07,
              "vis_lo": 2, "vis_hi": 4, "spawn_frac_hi": 0.10},
    "occ48": {"occ_lo": 48, "occ_hi": 48, "v_lo": 0.05, "v_hi": 0.07,
              "vis_lo": 2, "vis_hi": 4, "spawn_frac_hi": 0.10},
    # unseen velocities (faster than any training episode)
    "fast_v": {"v_lo": 0.11, "v_hi": 0.14},
    "slow_v": {"v_lo": 0.02, "v_hi": 0.035},
    # motion-model shifts
    "drag_x3": {"drag": 0.015},
    "v_flip": {"v_flip_prob": 0.6},
    # distractor stress
    "distractors4": {"distractor_probs": (0.0, 0.0, 0.0, 0.0, 1.0)},
    "ambush": {"distractor_probs": (0.0, 0.0, 0.0, 0.0, 1.0),
               "ambush_prob": 0.7},
    # appearance stress
    "app_jitter": {"reappear_app_jitter": 0.10},
    "swap_half": {"p_swap": 0.5},
}


def preset_params(p: dyn.EnvParams, preset: str) -> dyn.EnvParams:
    if preset not in PRESETS:
        raise ValueError(f"unknown preset {preset}; choices {sorted(PRESETS)}")
    overrides = dict(PRESETS[preset])
    if "distractor_probs" in overrides:
        overrides["distractor_probs"] = tuple(overrides["distractor_probs"])
    return dataclasses.replace(p, **overrides)


def intervention_steps(data: dict, at: str) -> np.ndarray:
    """Per-episode step index at which the intervention fires (-1 = never)."""
    if at == "mid_occl":
        return data["mid_occl"]
    if at == "occl_start":
        return np.where(data["t_occl"] >= 0, data["t_occl"] + 1, -1)
    if at == "mid_vis":
        return data["mid_vis"]
    if at == "pre_reappear":
        return np.where(data["t_reapp"] > 0, data["t_reapp"] - 1, -1)
    raise ValueError(f"unknown intervention anchor {at}")


# ---------------------------------------------------------------------------
# Torch model evaluation
# ---------------------------------------------------------------------------


def predict_model(model, data: dict, intervention: Optional[Dict] = None,
                  device: str = "cpu", seed: int = 0) -> dict:
    """Run a model over a generated batch. Returns prediction arrays."""
    import torch

    intervention = intervention or {"type": "none"}
    obs = torch.as_tensor(data["obs"], dtype=torch.float32, device=device)
    T, B, _ = obs.shape
    state = model.initial_state(B, device)
    target = torch.as_tensor(
        intervention_steps(data, intervention.get("at", "mid_occl")),
        dtype=torch.long, device=device,
    )
    gen = torch.Generator(device="cpu").manual_seed(seed)
    pos = torch.zeros(T, B)
    vel = torch.zeros(T, B)
    exist = torch.zeros(T, B)
    same = torch.zeros(T, B)
    with torch.no_grad():
        for t in range(T):
            fire = target == t
            if intervention["type"] == "hidden_reset" and bool(fire.any()):
                state = state.clone()
                state[fire] = 0.0
            elif intervention["type"] == "hidden_noise" and bool(fire.any()):
                sigma = float(intervention.get("sigma", 1.0))
                noise = torch.randn(state[fire].shape, generator=gen)
                state = state.clone()
                state[fire] += sigma * noise.to(device)
            p, e, i, state = model(obs[t], state)
            pos[t] = p[:, 0].cpu()
            vel[t] = p[:, 1].cpu()
            exist[t] = torch.sigmoid(e).cpu()
            same[t] = torch.sigmoid(i).cpu()
    return {"pos": pos.numpy(), "vel": vel.numpy(),
            "exist": exist.numpy(), "same": same.numpy()}


def evaluate_model(model, p: dyn.EnvParams, n_episodes: int, seed: int,
                   intervention: Optional[Dict] = None, device: str = "cpu",
                   keep_data: bool = False) -> dict:
    rng = np.random.default_rng(seed)
    data = dyn.generate_batch(p, n_episodes, rng)
    pred = predict_model(model, data, intervention, device=device, seed=seed)
    out = {
        "metrics": metrics_mod.compute_metrics(pred, data, p),
        "boutlen_curve": metrics_mod.curve_by_boutlen(pred, data),
    }
    if keep_data:
        out["pred"] = pred
        out["data"] = data
    return out


# ---------------------------------------------------------------------------
# Heuristic evaluation
# ---------------------------------------------------------------------------


def predict_heuristic(name: str, data: dict, p: dyn.EnvParams,
                      intervention: Optional[Dict] = None,
                      seed: int = 0) -> dict:
    from .agents.heuristics import make_heuristic

    intervention = intervention or {"type": "none"}
    obs = data["obs"]
    T, B, _ = obs.shape
    target = intervention_steps(data, intervention.get("at", "mid_occl"))
    rng = np.random.default_rng(seed)
    pos = np.zeros((T, B))
    vel = np.zeros((T, B))
    exist = np.zeros((T, B))
    same = np.zeros((T, B))
    for b in range(B):
        h = make_heuristic(name, p)
        h.reset()
        for t in range(T):
            if target[b] == t:
                if intervention["type"] == "hidden_reset":
                    h.reset_memory()
                elif intervention["type"] == "hidden_noise":
                    h.last_x += float(rng.normal(0.0, intervention.get("sigma", 0.05)))
                    h.last_v += float(rng.normal(0.0, intervention.get("sigma", 0.5)))
            x_hat, v_hat, e, s = h.predict(obs[t, b])
            pos[t, b] = x_hat
            vel[t, b] = v_hat
            exist[t, b] = e
            same[t, b] = s
    return {"pos": pos, "vel": vel, "exist": exist, "same": same}


def evaluate_heuristic(name: str, p: dyn.EnvParams, n_episodes: int,
                       seed: int, intervention: Optional[Dict] = None) -> dict:
    rng = np.random.default_rng(seed)
    data = dyn.generate_batch(p, n_episodes, rng)
    pred = predict_heuristic(name, data, p, intervention, seed=seed)
    return {
        "metrics": metrics_mod.compute_metrics(pred, data, p),
        "boutlen_curve": metrics_mod.curve_by_boutlen(pred, data),
    }


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--checkpoint", default=None)
    ap.add_argument("--heuristic", default=None,
                    help="evaluate a heuristic instead of a checkpoint")
    ap.add_argument("--episodes", type=int, default=None)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--preset", default="id", choices=sorted(PRESETS))
    ap.add_argument("--intervention", default="none",
                    choices=["none", "hidden_reset", "hidden_noise"])
    ap.add_argument("--at", default="mid_occl")
    ap.add_argument("--sigma", type=float, default=1.0)
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    cfg = load_config(args.config)
    if args.episodes:
        cfg.eval.episodes = args.episodes
    if args.seed is not None:
        cfg.eval.seed = args.seed
    p = preset_params(cfg.env, args.preset)
    intervention = {"type": args.intervention, "at": args.at,
                    "sigma": args.sigma}

    t0 = time.time()
    if args.heuristic:
        result = evaluate_heuristic(
            args.heuristic, p, cfg.eval.episodes, cfg.eval.seed, intervention)
        result["kind"] = args.heuristic
    else:
        import torch
        from .models import make_model

        arch, hidden = cfg.model.arch, cfg.model.hidden
        cp = None
        if args.checkpoint:
            cp = torch.load(args.checkpoint, map_location="cpu",
                            weights_only=True)
            spec = cp.get("model_spec") or {}
            arch = spec.get("arch", arch)
            hidden = spec.get("hidden", hidden)
        model = make_model(arch, hidden=hidden)
        if cp is not None:
            model.load_state_dict(cp["model"])
        model.eval()
        result = evaluate_model(model, p, cfg.eval.episodes, cfg.eval.seed,
                                intervention)
        result["kind"] = arch
        result["checkpoint"] = args.checkpoint
    result["preset"] = args.preset
    result["intervention"] = intervention
    result["episodes"] = cfg.eval.episodes
    result["wall_time_s"] = round(time.time() - t0, 2)

    text = json.dumps(metrics_mod.json_safe(result), indent=2)
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(text)
        print(f"wrote {args.out}")
    else:
        print(text)


if __name__ == "__main__":
    main()
