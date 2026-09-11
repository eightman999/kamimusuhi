"""T0 evaluation: delay generalization, hidden-state interventions, probes.

Splits (spec section 12):
  seen          training grid delays
  interpolation unseen delays inside the trained range
  extrapolation unseen delays beyond the trained range
  distractor    raised distractor rate
  scaled        world dynamics at x0.5 / x2.0 (T-C5)

Causal tests (spec section 9):
  T-C1 hidden reset   zero the hidden state mid-delay
  T-C2 hidden noise   add gaussian noise to the hidden state mid-delay
  T-C3 obs blank      replace delay-period observations with a constant
  T-C4 distractors    denser distractor pulses during the delay
  T-C5 time scaling   change world dynamics speed; correct ACT step moves
"""
import argparse
import json
from pathlib import Path

import numpy as np
import torch

from experiments.t0.analysis.plots import (plot_delay_curve, plot_interventions,
                                           plot_probe_bars)
from experiments.t0.analysis.probes import fit_time_probes
from experiments.t0.analysis.trajectories import per_delay_trajectories
from experiments.t0.env.interval_env import (EXTRAP_DELAYS, INTERP_DELAYS,
                                             SEEN_DELAYS, IntervalEnv)
from experiments.t0.models import make_model
from experiments.t0.train import atomic, collect, episode_metrics

EVAL_SEED = 900001


def load_model(checkpoint, device="cpu"):
    cp = torch.load(checkpoint, map_location=device, weights_only=False)
    config = cp.get("config", {})
    model = make_model(config["architecture"], num_actions=config.get("num_actions", 3))
    model.load_state_dict(cp["model"])
    model.to(device).eval()
    return model, config, cp


def _delay_env(num_envs, device, delay, config, **overrides):
    envconfig = dict(config.get("env", {}))
    envconfig.update(overrides)
    envconfig["horizon"] = int(envconfig.get("horizon", 160))
    need = int(round(delay / envconfig.get("time_scale", 1.))) + 16
    envconfig["horizon"] = max(envconfig["horizon"], need)
    env = IntervalEnv(num_envs, device, EVAL_SEED + int(delay), envconfig)
    return env


def eval_delay(model, device, delay, n=128, config=None, intervention=None,
               record_states=False, **env_overrides):
    """Greedy rollout on a single fixed delay; returns metrics dict."""
    env = _delay_env(n, device, delay, config or {}, **env_overrides)
    out = collect(model, env, greedy=True, intervention=intervention,
                  record_states=record_states)
    rollout, mask, infos = out[:3]
    metrics = episode_metrics(rollout, mask, infos, env)
    metrics["delay"] = int(delay)
    metrics["episodes"] = n
    if record_states:
        metrics["states"] = out[3]
        metrics["target_steps"] = env.target_step.clone()
        metrics["delays"] = env.delay.clone()
    return metrics


def hidden_reset(at_fraction=.5):
    def intervention(t, state, observation, env):
        hit = t == torch.round(env.target_step.float() * at_fraction).long()
        if bool(hit.any()) and state.numel():
            state = state.clone()
            state[hit] = 0.
        return state, observation
    return intervention


def hidden_noise(sigma=.5, at_fraction=.5, seed=0):
    gen = torch.Generator(device="cpu").manual_seed(seed)

    def intervention(t, state, observation, env):
        hit = t == torch.round(env.target_step.float() * at_fraction).long()
        if bool(hit.any()) and state.numel():
            noise = torch.randn(state[hit].shape, generator=gen).to(state.device)
            state = state.clone()
            state[hit] = state[hit] + sigma * noise
        return state, observation
    return intervention


def obs_blank(start=0., end=1.):
    """Constant observation over the fractional delay interval [start, end)."""
    def intervention(t, state, observation, env):
        lo = torch.round(env.target_step.float() * start).long()
        hi = torch.round(env.target_step.float() * end).long()
        during = (t >= lo) & (t < hi)
        if bool(during.any()):
            observation = observation.clone()
            observation[during] = .5
        return state, observation
    return intervention


def eval_with_intervention(model, device, delay, n, config, intervention_fn,
                           **env_overrides):
    env = _delay_env(n, device, delay, config, **env_overrides)
    def bound(t, state, observation):
        return intervention_fn(t, state, observation, env)
    rollout, mask, infos = collect(model, env, greedy=True, intervention=bound)[:3]
    metrics = episode_metrics(rollout, mask, infos, env)
    metrics["delay"] = int(delay)
    metrics["episodes"] = n
    return metrics


def run_probes(model, device, config, n=128, delays=SEEN_DELAYS):
    """Linear probes over alive delay-period steps pooled across delays."""
    states_all, targets_all, mask_all = [], [], []
    t_max = 0
    for d in delays:
        env = _delay_env(n, device, d, config)
        rollout, mask, infos, states = collect(model, env, greedy=True,
                                               record_states=True)
        if states.shape[-1] == 0:
            return {"note": "memoryless model: no hidden state to probe"}
        states_all.append(states)
        targets_all.append(env.target_step.clone())
        mask_all.append(mask)
        t_max = max(t_max, states.shape[0])
    for i, states in enumerate(states_all):
        if states.shape[0] < t_max:
            pad = t_max - states.shape[0]
            states_all[i] = torch.cat([states, torch.zeros(
                pad, states.shape[1], states.shape[2], device=states.device)])
            mask_all[i] = torch.cat([mask_all[i], torch.zeros(
                pad, mask_all[i].shape[1], dtype=torch.bool,
                device=mask_all[i].device)])
    states = torch.cat(states_all, dim=1)
    targets = torch.cat(targets_all)
    mask = torch.cat(mask_all, dim=1)
    return fit_time_probes(states, targets, mask=mask)


def evaluate_checkpoint(checkpoint, artifacts, device="cpu", episodes=128,
                        aux_episodes=64, seed=0):
    model, config, cp = load_model(checkpoint, device)
    out = {"checkpoint": str(checkpoint), "seed": config.get("seed"),
           "architecture": config.get("architecture"),
           "model_sha256": cp.get("model_sha256"), "episodes": episodes}
    sets = {"seen": SEEN_DELAYS, "interpolation": INTERP_DELAYS,
            "extrapolation": EXTRAP_DELAYS}
    for name, delays in sets.items():
        rows = {}
        for d in delays:
            rows[str(d)] = eval_delay(model, device, d, n=episodes, config=config)
        out[name] = rows
        out[name + "_mean_success"] = float(np.mean([r["success"] for r in rows.values()]))
    out["distractor"] = {str(d): eval_delay(model, device, d, n=episodes,
                                          config=config, distractor_rate=.2)
                         for d in SEEN_DELAYS}
    out["scaled"] = {}
    for s in (.5, 2.):
        out["scaled"][str(s)] = {str(d): eval_delay(model, device, d, n=episodes,
                                                  config=config, time_scale=s)
                                 for d in SEEN_DELAYS}
    out["interventions"] = {}
    mid = SEEN_DELAYS[len(SEEN_DELAYS) // 2]
    base = eval_delay(model, device, mid, n=episodes, config=config)
    out["interventions"]["baseline"] = base
    out["interventions"]["hidden_reset"] = eval_with_intervention(
        model, device, mid, episodes, config, hidden_reset(.5))
    out["interventions"]["hidden_noise"] = eval_with_intervention(
        model, device, mid, episodes, config, hidden_noise(.5, .5, seed))
    out["interventions"]["obs_blank"] = eval_with_intervention(
        model, device, mid, episodes, config, obs_blank(0., 1.))
    out["probes"] = run_probes(model, device, config, n=aux_episodes)
    traj_env = _delay_env(aux_episodes, device, mid, config)
    rollout, mask, infos, states = collect(model, traj_env, greedy=True,
                                           record_states=True)
    out["trajectory"] = per_delay_trajectories(states, traj_env.delay) \
        if states.shape[-1] else {"explained": [], "trajectories": {}}
    run_name = Path(checkpoint).parent.name + "_" + Path(checkpoint).stem
    dest = Path(artifacts) / "eval" / (run_name + ".json")
    save = {k: v for k, v in out.items() if k != "trajectory"}
    atomic(dest, save)
    plot_delay_curve(out["seen"], Path(artifacts) / "eval" / (run_name + "_seen_curve.png"))
    plot_probe_bars(out["probes"], Path(artifacts) / "eval" / (run_name + "_probes.png"))
    plot_interventions(
        [{"label": k, "success": v["success"]}
         for k, v in out["interventions"].items()],
        Path(artifacts) / "eval" / (run_name + "_interventions.png"))
    atomic(Path(artifacts) / "eval" / (run_name + "_trajectory.json"),
           out["trajectory"])
    return out


def consolidate(artifacts):
    """Per-seed stats across runs: mean/SD/median/bootstrap CI/sign test."""
    eval_dir = Path(artifacts) / "eval"
    rows = [json.loads(p.read_text()) for p in sorted(eval_dir.glob("*.json"))
            if not p.name.endswith("_trajectory.json")]
    by_arch = {}
    for row in rows:
        by_arch.setdefault(row["architecture"], []).append(row)
    rng = np.random.default_rng(0)
    summary = {}
    for arch, items in by_arch.items():
        def stat(values):
            values = np.asarray([v for v in values if v == v], dtype=float)
            if len(values) == 0:
                return {"n": 0, "mean": None, "sd": None, "median": None,
                        "ci95": [None, None]}
            boot = rng.choice(values, (2000, len(values))).mean(1)
            return {"n": len(values), "mean": float(values.mean()),
                    "sd": float(values.std(ddof=1)) if len(values) > 1 else 0.,
                    "median": float(np.median(values)),
                    "ci95": [float(np.percentile(boot, 2.5)),
                             float(np.percentile(boot, 97.5))]}
        summary[arch] = {
            "seeds": [r.get("seed") for r in items],
            "seen": stat([r["seen_mean_success"] for r in items]),
            "interpolation": stat([r["interpolation_mean_success"] for r in items]),
            "extrapolation": stat([r["extrapolation_mean_success"] for r in items]),
            "hidden_reset_success": stat(
                [r["interventions"]["hidden_reset"]["success"] for r in items]),
            "baseline_success": stat(
                [r["interventions"]["baseline"]["success"] for r in items]),
            "elapsed_r2": stat([r["probes"].get("elapsed_r2", float("nan"))
                                for r in items]),
        }
    recurrent = [a for a in summary if a != "mlp"]
    mlp_seen = np.array([r["seen_mean_success"] for r in by_arch.get("mlp", [])])
    for arch in recurrent:
        diffs = np.array([r["seen_mean_success"] for r in by_arch[arch]])
        paired = min(len(diffs), len(mlp_seen))
        if paired:
            delta = diffs[:paired] - mlp_seen[:paired]
            wins = int((delta > 0).sum())
            p_one = float(np.mean(rng.binomial(paired, .5, 20000) >= wins))
            summary[arch]["vs_mlp"] = {
                "mean_delta": float(delta.mean()),
                "sign_test_wins": wins, "sign_test_n": paired,
                "sign_test_p": float(min(1., 2 * min(p_one, 1 - p_one)))}
    atomic(Path(artifacts) / "consolidated.json", summary)
    return summary


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--checkpoint")
    p.add_argument("--artifacts", required=True)
    p.add_argument("--episodes", type=int, default=128)
    p.add_argument("--device", default="cpu")
    p.add_argument("--consolidate-only", action="store_true")
    a = p.parse_args()
    if a.consolidate_only:
        print(json.dumps(consolidate(a.artifacts), indent=2))
        return
    out = evaluate_checkpoint(a.checkpoint, a.artifacts, a.device, a.episodes)
    print(json.dumps({k: v for k, v in out.items() if k != "trajectory"},
                     indent=2))


if __name__ == "__main__":
    main()
