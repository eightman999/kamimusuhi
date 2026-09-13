"""Evaluation for U0: learned checkpoints and heuristic baselines.

Runs a fixed, CPU-generated evaluation corpus (seed 900001 by default,
held out from training and validation seeds) and reports the primary
metrics: homeostatic error (alive and full-episode), survival, need
resolution, important retention and store precision.

Causal tests (--causal), applied just before each need onset:
    erase               U-C1: clear memory entirely (blanket; used by the
                        protocol sanity gate)
    targeted_erase      U-H3: remove only the stored items whose function
                        serves the need about to fire
    targeted_mediation  U-H4b: same manipulation as targeted_erase,
                        reported as its own arm for the mediation test
    donor_shuffle       U-H3: swap memory contents with a parallel donor
                        episode

Counterfactual probe (--probe need_intervention), U-C3/U-H4: re-run
episodes with the internal state pinned to a fixed profile AND slot
memory cleared each step during the event window, so world seed,
observation stream, event, and memory state are identical across
conditions — only the internal need state differs. The headline
statistic is the mean absolute shift in P(STORE | event function)
between an adverse internal state and its paired safe state.

Encoding-permutation test (--ood event_permutation), U-C4: permute event type
and location channels in the observation; semantics are unchanged, so a
policy that memorized channel positions breaks.

OOD (--ood): delay96/delay128/delay160, distractor2x/distractor4x,
need_mapping_shift, event_permutation, capacity2.

Usage:
    python -m experiments.u0.evaluate --checkpoint RUN/best.pt --episodes 256
    python -m experiments.u0.evaluate --baseline fifo --episodes 256
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path

import numpy as np
import torch

from .config import env_config, load_config, resolve_device
from .env.u0_env import (STORE, INTERNAL_NAMES, FUNCTION_NAMES, U0Config)
from .policies.baselines import BASELINES, FIFOPolicy

EVAL_SEED = 900001

OOD_OVERRIDES = {
    "delay96": {"delay_min": 96, "delay_max": 96, "episode_len": 208},
    "delay128": {"delay_min": 128, "delay_max": 128, "episode_len": 240},
    "delay160": {"delay_min": 160, "delay_max": 160, "episode_len": 272},
    "distractor2x": {"n_distractors": 8},
    "distractor4x": {"n_distractors": 16},
    # U-C5b semantic rotation: sites of function f heal variable (f+1)%4.
    # Event reports stay truthful; the type<->variable association shifts.
    "need_mapping_shift": {"function_shift": (1, 2, 3, 0)},
    # U-C4: scramble the observed channel positions (semantics intact)
    "event_permutation": {"obs_func_perm": (2, 0, 3, 1),
                   "obs_loc_perm": (1, 4, 0, 3, 2, 6, 8, 5, 10, 7,
                                    11, 9)},
    # finite-capacity stress: only 2 slots for 2 planned needs
    "capacity2": {"memory_slots": 2},
}
OOD_MODES = list(OOD_OVERRIDES)
CAUSAL_MODES = ["erase", "targeted_erase", "donor_shuffle",
                "targeted_mediation"]

# counterfactual internal-state profiles for the need-intervention probe.
# every variable is pinned to a comfortable mid value except the named
# one, which is pushed outside its preferred range.
_SAFE_PROFILE = {"energy": 0.65, "temperature": 0.50, "risk": 0.10,
                 "certainty": 0.70}
NEED_CONDITIONS = {
    "all_safe": {},
    "energy_low": {"energy": 0.25},
    "energy_safe": {"energy": 0.65},
    "temperature_low": {"temperature": 0.28},
    "temperature_safe": {"temperature": 0.50},
    "risk_high": {"risk": 0.45},
    "risk_low": {"risk": 0.10},
    "certainty_low": {"certainty": 0.25},
    "certainty_safe": {"certainty": 0.70},
}
# (adverse condition, safe condition) paired per function
_CONDITION_PAIRS = {
    "resource": ("energy_low", "energy_safe"),
    "shelter": ("temperature_low", "temperature_safe"),
    "safe_zone": ("risk_high", "risk_low"),
    "obs_point": ("certainty_low", "certainty_safe"),
}


def make_ood_config(cfg: U0Config, ood: str | None) -> U0Config:
    if ood is None:
        return cfg
    d = {**cfg.__dict__, **OOD_OVERRIDES[ood]}
    return U0Config(**d)


def _apply_causal(env, donor, causal: str | None) -> None:
    """Fire the manipulation inside the window between the relevant
    event being stored and the need onset (t == onset step, before the
    step activates the need).

    targeted_erase / targeted_mediation remove only the stored items
    whose function serves the need about to fire — the memories the
    agent kept *for* this need. donor_shuffle swaps the whole memory
    with a parallel donor episode. erase clears everything."""
    if causal is None:
        return
    firing = [n for n in env.needs
              if n.onset == env.t and not n.active and not n.resolved]
    if not firing:
        return
    if causal == "erase":
        env.erase_memory()
    elif causal in ("targeted_erase", "targeted_mediation"):
        for n in firing:
            env.erase_for_need(n.var)
    elif causal == "donor_shuffle" and donor is not None:
        env.swap_memory(donor.memory.clone_state())


def _mean_stats(records: list[dict]) -> dict:
    keys = records[0].keys()
    out = {}
    for k in keys:
        vals = [r[k] for r in records]
        nums = [float(v) for v in vals
                if isinstance(v, (int, float, np.floating, np.integer))]
        if len(nums) == len(vals):          # skip non-numeric (death_cause)
            out[k] = float(np.mean(nums))
    return out


class PolicyWrapper:
    """Wrap a learned nn.Module policy as act(obs)->int (greedy)."""

    def __init__(self, model, device: str = "cpu"):
        self.model = model
        self.device = device
        self.h = None

    def reset(self) -> None:
        self.h = None

    def act(self, obs: np.ndarray) -> int:
        with torch.no_grad():
            x = torch.as_tensor(obs, dtype=torch.float32,
                                device=self.device).unsqueeze(0)
            logits, _v, self.h = self.model(
                x, self.h if self.h is not None else
                self.model.initial_state(1, self.device))
            return int(logits.argmax(-1).item())


def _fresh_donor(cfg: U0Config, seed: int):
    from .env.u0_env import U0Env
    donor = U0Env(U0Config(**{**cfg.__dict__, "seed": seed}))
    gate = FIFOPolicy(seed=seed)
    return donor, gate


def evaluate_learned(model, cfg: U0Config, device: str = "cpu",
                     episodes: int = 256, seed: int = EVAL_SEED,
                     causal: str | None = None,
                     batch: int = 32) -> dict:
    from .env.u0_env import U0Env
    dev = torch.device(resolve_device(device))
    model.to(dev)
    model.eval()
    records = []
    for ep0 in range(0, episodes, batch):
        n = min(batch, episodes - ep0)
        envs = [U0Env(U0Config(**{**cfg.__dict__,
                                  "seed": seed + ep0 * 97 + i}))
                for i in range(n)]
        donors = donor_gates = None
        if causal == "donor_shuffle":
            donors, donor_gates = [], []
            for i in range(n):
                d, g = _fresh_donor(cfg, seed + 500000 + ep0 * 97 + i)
                donors.append(d)
                donor_gates.append(g)
        for e in envs:
            e.reset()
        if donors:
            for d in donors:
                d.reset()
        h = model.initial_state(n, dev)
        for _t in range(cfg.episode_len):
            idxs = [i for i, e in enumerate(envs) if not e.done]
            for i in idxs:
                _apply_causal(envs[i], donors[i] if donors else None,
                              causal)
            if donors:
                for d, g in zip(donors, donor_gates):
                    if not d.done:
                        d.step(g.decide(d))
            if not idxs:
                break
            obs = np.stack([envs[i]._obs() for i in idxs])
            with torch.no_grad():
                h_in = h[idxs] if h is not None else None
                logits, _v, h_out = model(
                    torch.as_tensor(obs, device=dev), h_in)
                acts = logits.argmax(-1).cpu().numpy()
                if h is not None:
                    h[idxs] = h_out
            for i, a in zip(idxs, acts):
                _o, _r, done, info = envs[i].step(int(a))
                if done and "ep_stats" in info:
                    records.append(info["ep_stats"])
    return _mean_stats(records)


def evaluate_baseline(name: str, cfg: U0Config, episodes: int = 256,
                      seed: int = EVAL_SEED,
                      causal: str | None = None) -> dict:
    from .env.u0_env import U0Env
    gate = BASELINES[name](seed=seed)
    env = U0Env(U0Config(**{**cfg.__dict__, "seed": seed,
                           "evict_policy": gate.evict_policy}))
    donor = donor_gate = None
    if causal == "donor_shuffle":
        donor, donor_gate = _fresh_donor(cfg, seed + 500000)
    records = []
    for ep in range(episodes):
        env.reset()
        gate.reset()
        if donor is not None:
            donor.reset()
            donor_gate.reset()
        for _t in range(cfg.episode_len):
            if env.done:
                break
            _apply_causal(env, donor, causal)
            if donor is not None and not donor.done:
                donor.step(donor_gate.decide(donor))
            _o, _r, done, info = env.step(gate.decide(env))
            if done:
                records.append(info["ep_stats"])
    return _mean_stats(records)


# ---------------------------------------------------------------------------
# U-C3 need-state intervention probe
# ---------------------------------------------------------------------------


def _pin_internal(env, profile: dict) -> None:
    """Counterfactual need state: pin the internal channels AND the
    vulnerability profile so both reflect the same counterfactual —
    the only thing differing between probe conditions."""
    from .env.u0_env import N_INTERNAL, var_deviation
    for i, name in enumerate(INTERNAL_NAMES):
        env.internal[i] = profile.get(name, _SAFE_PROFILE[name])
    env.vuln = np.array([var_deviation(env.internal, i)
                         for i in range(N_INTERNAL)],
                        dtype=np.float32)


def need_intervention_probe(model, cfg: U0Config, device: str = "cpu",
                            episodes: int = 128,
                            seed: int = 910001) -> dict:
    """Measure P(STORE | event function) under counterfactual need states.

    Identical event streams; only the pinned internal profile differs.
    Returns per-condition store probabilities and the mean absolute
    shift between each adverse state and its paired safe state.
    """
    from .env.u0_env import U0Env, CLS_FUNCTIONAL
    dev = torch.device(resolve_device(device))
    model.to(dev)
    model.eval()
    cond_results: dict = {}
    for cond, pin in NEED_CONDITIONS.items():
        profile = {**_SAFE_PROFILE, **pin}
        stores_by_func = {f: 0 for f in FUNCTION_NAMES}
        seen_by_func = {f: 0 for f in FUNCTION_NAMES}
        total_stores = total_events = 0
        for ep in range(episodes):
            env = U0Env(U0Config(**{**cfg.__dict__, "seed": seed + ep}))
            env.reset()
            pol = PolicyWrapper(model, str(dev))
            pol.reset()
            last_func_t = max(
                (t for t, e in enumerate(env.schedule)
                 if e.cls == CLS_FUNCTIONAL), default=0)
            for t in range(last_func_t + 1):
                _pin_internal(env, profile)   # counterfactual state
                env.erase_memory()            # identical memory channel
                e = env.schedule[env.t]
                obs = env._obs()
                a = pol.act(obs)
                if e.cls == CLS_FUNCTIONAL:
                    fname = FUNCTION_NAMES[e.func]
                    seen_by_func[fname] += 1
                    total_events += 1
                    if a == STORE:
                        stores_by_func[fname] += 1
                        total_stores += 1
                env.step(a)
        cond_results[cond] = {
            "episodes": episodes,
            "store_prob_by_func": {
                f: (stores_by_func[f] / seen_by_func[f]
                    if seen_by_func[f] else float("nan"))
                for f in FUNCTION_NAMES},
            "store_prob_all": (total_stores / total_events
                               if total_events else float("nan")),
        }
    delta_by_func = {}
    for fname, (adv, safe) in _CONDITION_PAIRS.items():
        pa = cond_results[adv]["store_prob_by_func"][fname]
        ps = cond_results[safe]["store_prob_by_func"][fname]
        delta_by_func[fname] = abs(pa - ps)
    return {
        "conditions": cond_results,
        "delta_by_func": delta_by_func,
        "mean_abs_delta": float(np.mean(list(delta_by_func.values()))),
    }


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default="experiments/u0/configs/default.yaml")
    ap.add_argument("--checkpoint", default=None)
    ap.add_argument("--baseline", default=None, choices=list(BASELINES))
    ap.add_argument("--model", default="gru64")
    ap.add_argument("--episodes", type=int, default=256)
    ap.add_argument("--seed", type=int, default=EVAL_SEED)
    ap.add_argument("--causal", default=None, choices=CAUSAL_MODES)
    ap.add_argument("--ood", default=None, choices=OOD_MODES)
    ap.add_argument("--probe", default=None, choices=["need_intervention"])
    ap.add_argument("--device", default="cpu",
                    choices=["cpu", "mps", "auto"])
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    device = resolve_device(args.device)
    raw = load_config(args.config)
    cfg = make_ood_config(env_config(raw, seed=args.seed), args.ood)

    if args.probe == "need_intervention":
        from .models.nets import build_policy
        model = build_policy(args.model)
        if args.checkpoint:
            state = torch.load(args.checkpoint, map_location="cpu",
                               weights_only=True)
            model.load_state_dict(state["model"])
        result = need_intervention_probe(model, cfg, device,
                                         episodes=args.episodes,
                                         seed=args.seed)
        result.update({"probe": args.probe, "ood": args.ood})
    elif args.baseline:
        metrics = evaluate_baseline(args.baseline, cfg, args.episodes,
                                    args.seed, args.causal)
        result = {"subject": f"baseline_{args.baseline}",
                  "causal": args.causal or "none",
                  "ood": args.ood or "none", "metrics": metrics}
    else:
        from .models.nets import build_policy
        model = build_policy(args.model)
        state = torch.load(args.checkpoint, map_location="cpu",
                           weights_only=True)
        model.load_state_dict(state["model"])
        metrics = evaluate_learned(model, cfg, device, args.episodes,
                                   args.seed, args.causal)
        result = {"subject": Path(args.checkpoint).parent.name,
                  "causal": args.causal or "none",
                  "ood": args.ood or "none", "metrics": metrics}

    text = json.dumps(result, indent=2)
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(text)
        print(f"wrote {args.out}")
    else:
        print(text)


if __name__ == "__main__":
    main()
