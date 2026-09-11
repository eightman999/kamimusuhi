"""U0 evaluation battery.

Subjects: every policy in policies/ (learned, gated baselines, degenerate
controls, Bayes oracle).  Conditions:

    clean            training distribution
    uc1_shuffle      U-C1: uncertainty signals permuted across episodes
    uc2_mask         U-C2: uncertainty replaced by constants
    uc3_noiseshift   U-C3: all sigmas x noise_scale_eval (calibration shift)
    uc4_prior        U-C4: class prior -> skewed (label-frequency shift)
    uc5_high/low     U-C5: injected false confidence / false doubt
    ood_*            unseen noise level, rotated basis, missing sensors,
                     correlated-noise pattern

Usage:
    python -m experiments.u0.evaluate --run-dir artifacts/runs/s0 \
        --config experiments/u0/configs/default.yaml
    python -m experiments.u0.evaluate --validate-env-only
"""

from __future__ import annotations

import argparse
import json
import time
from dataclasses import asdict
from pathlib import Path

import numpy as np
import torch
import yaml

from .analysis import metrics as met
from .env.uncertainty_env import U0Config, VecU0Env
from .models.nets import (Classifier, ConfNet, DropoutClassifier,
                          MetaPolicy, U_DIM)
from .policies.base import Policy, perturb_u
from .policies.baselines import (AlwaysAbstain, AlwaysAnswer,
                                 AlwaysObserve, ConfGated, EnsemblePolicy,
                                 EntropyGate, LearnedPolicy,
                                 MCDropoutPolicy, RandomPolicy,
                                 ThresholdPolicy, tune_gates)
from .policies.oracle import BayesOracle

EVAL_SEED = 900001

CAUSAL_MODES = ["uc1_shuffle", "uc2_mask", "uc5_high", "uc5_low"]
OOD_MODES = ["noise", "shift", "missing", "pattern"]
ENV_SHIFT_MODES = {                    # env-level shifts (not u edits)
    "uc3_noiseshift": {"noise_scale": 1.6},
    "uc4_prior": {"class_prior": (0.7, 0.1, 0.1, 0.1)},
}


# ---------------------------------------------------------------------------
# batched rollout
# ---------------------------------------------------------------------------

def run_eval(policy, cfg: U0Config, episodes: int = 256,
             seed: int = EVAL_SEED, device: str = "cpu", batch: int = 64,
             perturb: str | None = None, keep_records: bool = True) -> dict:
    """Run `episodes` episodes under an optional u-perturbation."""
    t0 = time.time()
    rng = torch.Generator().manual_seed(seed + 99)
    np_rng = np.random.default_rng(seed + 7)
    records: list[dict] = []
    batched = getattr(policy, "batched", True)
    n_waves = int(np.ceil(episodes / batch))
    for wave in range(n_waves):
        vec = VecU0Env(cfg, batch, seed=seed + wave * 10_000)
        obs = torch.as_tensor(vec.reset(), dtype=torch.float32,
                              device=device)
        alive = np.ones(batch, dtype=bool)
        finals: dict = {}
        for _t in range(cfg.max_obs):
            if not alive.any():
                break
            u_np = np.zeros((batch, 4))
            if batched:
                with torch.no_grad():
                    u = policy.u(obs)
                    u_used = perturb_u(u, perturb, rng) \
                        if getattr(policy, "uses_u", True) else u
                    acts = policy.decide(obs, u_used).cpu().numpy()
                    probs = policy.probs(obs)
                u_np = u.cpu().numpy()
            else:
                acts = np.zeros(batch, dtype=np.int64)
                probs = torch.zeros(batch, cfg.n_classes)
                for i, e in enumerate(vec.envs):
                    if not alive[i] or e.done:
                        continue
                    acts[i] = policy.decide_env(e)
                    probs[i] = torch.as_tensor(policy._post,
                                               dtype=torch.float32)
                    u_np[i] = policy.u_env(e)
            next_obs = np.zeros((batch, cfg.obs_dim))
            for i, e in enumerate(vec.envs):
                if not alive[i] or e.done:
                    continue
                a = int(acts[i])
                # snapshot per-step info for the record
                rec = {"regime": e.latent["regime"], "y": e.latent["y"],
                       "n_obs": e.n, "probs": probs[i].tolist(),
                       "conf": float(u_np[i, 0])}
                o, r, d, info = e.step(a)
                next_obs[i] = o
                rec["last_action"] = a
                rec["reward"] = r
                if d:
                    er = info["ep_record"]
                    rec.update(outcome=er["outcome"],
                               n_obs=er["n_obs"],
                               observe_cost=er["observe_cost"],
                               action_counts=er["action_counts"],
                               correct=bool(info.get("correct", False)))
                    if rec["outcome"] != "answer":
                        rec["correct"] = False
                    finals[i] = rec
                    alive[i] = False
            obs = torch.as_tensor(next_obs, dtype=torch.float32,
                                  device=device)
        records.extend(finals[i] for i in sorted(finals))
    out = {"metrics": met.episode_metrics(records),
           "per_regime": met.per_regime(records),
           "wall_sec": round(time.time() - t0, 2)}
    if keep_records:
        out["records"] = records
    return out


# ---------------------------------------------------------------------------
# subjects
# ---------------------------------------------------------------------------

def load_supervised(run_dir: str | Path, device: str = "cpu"):
    ck = torch.load(Path(run_dir) / "supervised.pt", map_location=device,
                    weights_only=False)
    obs_dim, K, hid = ck["obs_dim"], ck["n_classes"], ck["hidden"]
    clf = Classifier(obs_dim, K, hid)
    clf.load_state_dict(ck["clf"])
    clf.eval()
    conf_net = ConfNet(obs_dim, hid)
    conf_net.load_state_dict(ck["conf"])
    conf_net.eval()
    members = []
    for sd in ck["ensemble"]:
        m = Classifier(obs_dim, K, hid)
        m.load_state_dict(sd)
        m.eval()
        members.append(m)
    dclf = DropoutClassifier(obs_dim, K, hid)
    dclf.load_state_dict(ck["dropout_clf"])
    dclf.eval()
    return {"clf": clf, "conf_net": conf_net, "ensemble": members,
            "dropout_clf": dclf, "env": ck["env"]}


def load_policy_ckpt(path: str | Path, device: str = "cpu"):
    ck = torch.load(path, map_location=device, weights_only=False)
    meta = MetaPolicy(ck["u_dim"], ck["hidden"])
    meta.load_state_dict(ck["meta"])
    meta.eval()
    clf = Classifier(ck["obs_dim"], ck["n_classes"], ck["clf_hidden"])
    clf.load_state_dict(ck["clf"])
    clf.eval()
    conf_net = ConfNet(ck["obs_dim"], ck["clf_hidden"])
    conf_net.load_state_dict(ck["conf"])
    conf_net.eval()
    return {"meta": meta, "clf": clf, "conf_net": conf_net, "ck": ck}


def build_subjects(run_dir: str | Path, cfg: U0Config,
                   device: str = "cpu", which: str = "best") -> dict:
    """Every subject of the eval matrix."""
    sup = load_supervised(run_dir, device)
    pol_ck = load_policy_ckpt(Path(run_dir) / f"policy_{which}.pt", device)
    clf, conf_net, meta = pol_ck["clf"], pol_ck["conf_net"], pol_ck["meta"]
    return {
        "learned": LearnedPolicy(clf, conf_net, meta, cfg),
        "always_answer": AlwaysAnswer(clf, cfg),
        "always_observe": AlwaysObserve(clf, cfg),
        "always_abstain": AlwaysAbstain(cfg),
        "random": RandomPolicy(clf, cfg, seed=0),
        "threshold": ThresholdPolicy(clf, cfg),
        "entropy": EntropyGate(clf, cfg),
        "ensemble": EnsemblePolicy(sup["ensemble"], cfg),
        "mcdropout": MCDropoutPolicy(sup["dropout_clf"], cfg),
        "conf_gated": ConfGated(clf, conf_net, cfg),
        "oracle": BayesOracle(cfg),
    }


GATED = ["threshold", "entropy", "ensemble", "mcdropout", "conf_gated"]


# ---------------------------------------------------------------------------
# environment validation (pre-learning sanity)
# ---------------------------------------------------------------------------

def validate_env(cfg: U0Config, episodes: int = 400,
                 seed: int = 31337) -> dict:
    """Oracle / random / degenerate baselines: env must be neither trivial
    nor unsolvable, and the oracle must use all three meta-actions."""
    out = {}
    for name in ("oracle", "always_answer", "always_observe",
                 "always_abstain", "random"):
        if name == "oracle":
            pol = BayesOracle(cfg)
        elif name == "always_answer":
            # difficulty-blind argmax-by-mean policy (no classifier yet):
            # answer the prototype nearest the sample mean
            pol = _NearestProtoPolicy(cfg)
        elif name == "always_observe":
            pol = _NearestProtoPolicy(cfg, observe_first=True)
        elif name == "always_abstain":
            pol = AlwaysAbstain(cfg)
        else:
            pol = _RandomFlatPolicy(cfg, seed)
        out[name] = run_eval(pol, cfg, episodes=episodes, seed=seed,
                             batch=32, keep_records=True)
    return out


class _NearestProtoPolicy(Policy):
    """Heuristic difficulty-blind baseline: answer argmin||xbar - mu_k||;
    optionally burn the observation budget first."""

    def __init__(self, cfg: U0Config, observe_first: bool = False):
        self.cfg, self.observe_first = cfg, observe_first
        from .env.uncertainty_env import U0Env
        e = U0Env(cfg)
        self.proto = e.proto
        self.proto_alt = e.proto_alt

    name = "nearest_proto"
    uses_u = False

    def u(self, obs):
        return torch.zeros(obs.shape[0], 4)

    def probs(self, obs):
        proto = self.proto_alt if self.cfg.ood == "shift" else self.proto
        d = torch.cdist(obs[:, :self.cfg.feat_dim],
                        torch.as_tensor(proto.T, dtype=torch.float32))
        return torch.softmax(-d, -1)

    def decide(self, obs, u):
        proto = self.proto_alt if self.cfg.ood == "shift" else self.proto
        d = torch.cdist(obs[:, :self.cfg.feat_dim],
                        torch.as_tensor(proto.T, dtype=torch.float32))
        ans = d.argmin(-1)
        n = obs[:, self.cfg.feat_dim + 1]
        if self.observe_first:
            return torch.where(
                n < 1.0, torch.full_like(ans, self.cfg.observe_id), ans)
        return ans


class _RandomFlatPolicy(Policy):
    name = "random_flat"
    uses_u = False

    def __init__(self, cfg: U0Config, seed: int):
        self.cfg = cfg
        self.gen = torch.Generator().manual_seed(seed)

    def u(self, obs):
        return torch.zeros(obs.shape[0], 4)

    def probs(self, obs):
        return torch.full((obs.shape[0], self.cfg.n_classes),
                          1.0 / self.cfg.n_classes)

    def decide(self, obs, u):
        return torch.randint(0, self.cfg.n_actions, (obs.shape[0],),
                             generator=self.gen)


# ---------------------------------------------------------------------------
# full battery
# ---------------------------------------------------------------------------

def eval_battery(run_dir: str | Path, cfg: U0Config, episodes: int = 512,
                 aux_episodes: int = 256, seed: int = EVAL_SEED,
                 device: str = "cpu", out_dir: str | Path | None = None,
                 which: str = "best") -> dict:
    run_dir = Path(run_dir)
    subjects = build_subjects(run_dir, cfg, device, which)
    gated = [subjects[g] for g in GATED]
    t0 = time.time()
    tuned = tune_gates(gated, None, cfg, episodes=192, seed=seed + 4242,
                     device=device)
    print(f"tuned gates: {tuned}", flush=True)

    results: dict = {"run_dir": str(run_dir), "seed": seed,
                     "episodes": episodes, "aux_episodes": aux_episodes,
                     "tuned_gates": tuned, "env": asdict(cfg),
                     "subjects": {}}
    for name, pol in subjects.items():
        sub = {"clean": run_eval(pol, cfg, episodes=episodes, seed=seed,
                                 device=device)}
        for mode in CAUSAL_MODES:
            sub[mode] = run_eval(pol, cfg, episodes=aux_episodes,
                                 seed=seed + 100, device=device,
                                 perturb=mode)
        for sname, ov in ENV_SHIFT_MODES.items():
            c2 = U0Config(**{**cfg.__dict__, **ov})
            pol2 = BayesOracle(c2) if name == "oracle" else pol
            sub[sname] = run_eval(pol2 if name == "oracle" else pol, c2,
                                  episodes=aux_episodes, seed=seed + 200,
                                  device=device)
        for mode in OOD_MODES:
            c2 = U0Config(**{**cfg.__dict__, "ood": mode})
            pol2 = BayesOracle(c2) if name == "oracle" else pol
            sub[f"ood_{mode}"] = run_eval(
                pol2 if name == "oracle" else pol, c2,
                episodes=aux_episodes, seed=seed + 300, device=device)
        results["subjects"][name] = {
            k: {kk: vv for kk, vv in v.items() if kk != "records"}
            for k, v in sub.items()}
        # raw per-episode records for clean + OOD (kept for analysis)
        results["subjects"][name]["clean"]["records"] = sub["clean"]["records"]
        for mode in OOD_MODES:
            results["subjects"][name][f"ood_{mode}"]["records"] = \
                sub[f"ood_{mode}"]["records"]
        for mode in CAUSAL_MODES:
            results["subjects"][name][mode]["records"] = \
                sub[mode]["records"]
        results["subjects"][name]["uc3_noiseshift"]["records"] = \
            sub["uc3_noiseshift"]["records"]
        results["subjects"][name]["uc4_prior"]["records"] = \
            sub["uc4_prior"]["records"]
        print(f"  {name}: reward "
              f"{sub['clean']['metrics']['mean_reward']:.3f} "
              f"acc {sub['clean']['metrics']['accuracy']:.3f} "
              f"abstain {sub['clean']['metrics']['abstain_rate']:.3f}",
              flush=True)
    results["wall_sec"] = round(time.time() - t0, 2)
    if out_dir is not None:
        out_dir = Path(out_dir)
        out_dir.mkdir(parents=True, exist_ok=True)
        dest = out_dir / f"eval_{run_dir.name}_{which}.json"
        tmp = dest.with_suffix(".tmp")
        tmp.write_text(json.dumps(results, allow_nan=False))
        tmp.replace(dest)
        print(f"wrote {dest}", flush=True)
    return results


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--run-dir", default=None)
    ap.add_argument("--config", default="experiments/u0/configs/default.yaml")
    ap.add_argument("--episodes", type=int, default=512)
    ap.add_argument("--aux-episodes", type=int, default=256)
    ap.add_argument("--seed", type=int, default=EVAL_SEED)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--which", default="best", choices=["best", "final"])
    ap.add_argument("--out-dir", default=None)
    ap.add_argument("--validate-env-only", action="store_true")
    args = ap.parse_args()

    cfg = U0Config(**yaml.safe_load(open(args.config)).get("env", {}))
    if args.validate_env_only:
        res = validate_env(cfg)
        slim = {s: {"metrics": r["metrics"], "per_regime": r["per_regime"]}
                for s, r in res.items()}
        print(json.dumps(slim, indent=2, allow_nan=False))
        return
    res = eval_battery(args.run_dir, cfg, args.episodes,
                       args.aux_episodes, args.seed, args.device,
                       args.out_dir, args.which)
    slim = json.dumps({s: {c: v["metrics"] for c, v in cs.items()}
                       for s, cs in res["subjects"].items()}, indent=2)
    print(slim)


if __name__ == "__main__":
    main()
