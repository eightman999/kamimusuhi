"""X0 evaluation: retrieval, reconstruction, probes, causal tests, OOD.

Evaluation tasks (spec section 5):
  X0-A cross-modal retrieval        top1 / MRR / matching accuracy
  X0-B missing-modality reconstruction  cross-modal decode R^2 vs true partner
  X0-C shared-cause probe           linear probe; cross-modal transfer matrix
  X0-D false-pair rejection         conflict scenes + partner-absent control

Causal tests (spec section 6):
  X-C1 temporal desynchronization   lag sweep {0,1,2,4,8}
  X-C2 false synchrony              conflict scenes (delta in {1,2})
  X-C3 modality dropout             token-drop sweep + whole-modality removal
  X-C4 sensor transform             new appearance mapping (alt transforms)
  X-C5 cause recombination          held-out factor combinations

Baselines: chance, timing, concat, pca, cca (+ oracle upper bound).
Trained models are loaded from run checkpoints (best.pt).
"""

import argparse
import dataclasses
import hashlib
import itertools
import json
import time
from pathlib import Path

import numpy as np
import torch

from experiments.x0.analysis.metrics import (cohens_d, false_binding,
                                             matching_accuracy, retrieval,
                                             sign_test_p, summarise)
from experiments.x0.analysis.plots import (plot_false_binding, plot_lag_curve,
                                         plot_latent_pca, plot_probe_heatmap,
                                         plot_retrieval_bars)
from experiments.x0.config import Config, EnvConfig, EvalConfig, ModelSpec
from experiments.x0.env.latent_cause import LatentCauseEnv
from experiments.x0.models import make_model
from experiments.x0.models.baselines import (CcaMethod, ChanceMethod,
                                             ConcatMethod, PcaMethod,
                                             TimingMethod, cosine_sim)
from experiments.x0.probes.linear import cross_modal_matrix
from experiments.x0.train import atomic, collate, make_env

EVAL_SEED = 900001


# ----------------------------------------------------------------------
# methods
# ----------------------------------------------------------------------
class TorchMethod:
    """Trained torch model behind the method interface (features only)."""

    def __init__(self, model, device="cpu"):
        self.model = model.eval()
        self.device = device

    @property
    def name(self):
        return "torch"

    def embed(self, m, feats):
        with torch.no_grad():
            x = torch.as_tensor(feats, dtype=torch.float32, device=self.device)
            return self.model.embed(m, x).cpu().numpy()

    def pair_sim(self, m, rec_m, n, rec_n):
        return cosine_sim(self.embed(m, rec_m["feats"]),
                          self.embed(n, rec_n["feats"]))


class OracleMethod:
    """Upper bound: similarity = 1 iff same event (uses ground truth)."""
    name = "oracle"

    def pair_sim(self, m, rec_m, n, rec_n):
        same = rec_m["events"][:, None] == rec_n["events"][None, :]
        return same.astype(float)

    def embed(self, m, feats):
        raise NotImplementedError("oracle has no feature embedding")


def rec(em):
    return {"times": em.times, "feats": em.feats, "events": em.events}


# ----------------------------------------------------------------------
# core evaluators
# ----------------------------------------------------------------------
def eval_retrieval(method, scenes, modalities, pairs=None):
    """Macro top1/MRR/matching over ordered modality pairs."""
    pairs = pairs or [(a, b) for a, b in itertools.permutations(modalities, 2)]
    agg = {f"{a}->{b}": {"hit": 0.0, "rr": 0.0, "n": 0, "match": []}
           for a, b in pairs}
    for sc in scenes:
        for a, b in pairs:
            ra, rb = sc.emissions.get(a), sc.emissions.get(b)
            if ra is None or rb is None or len(ra.times) == 0 \
                    or len(rb.times) == 0:
                continue
            sim = method.pair_sim(a, rec(ra), b, rec(rb))
            partner = sc.partner_index(a, b)
            r = retrieval(sim, partner)
            agg[f"{a}->{b}"]["hit"] += r["top1"] * r["n"] if r["n"] else 0
            agg[f"{a}->{b}"]["rr"] += r["mrr"] * r["n"] if r["n"] else 0
            agg[f"{a}->{b}"]["n"] += r["n"]
            agg[f"{a}->{b}"]["match"].append(
                matching_accuracy(sim, ra.events, rb.events))
    out = {}
    top1s, mrrs, matches = [], [], []
    for k, v in agg.items():
        top1 = v["hit"] / v["n"] if v["n"] else float("nan")
        mrr = v["rr"] / v["n"] if v["n"] else float("nan")
        match = float(np.nanmean(v["match"])) if v["match"] else float("nan")
        out[k] = {"top1": top1, "mrr": mrr, "match": match, "n": v["n"]}
        top1s.append(top1)
        mrrs.append(mrr)
        matches.append(match)
    out["macro"] = {"top1": float(np.nanmean(top1s)),
                    "mrr": float(np.nanmean(mrrs)),
                    "match": float(np.nanmean(matches))}
    return out


def eval_conflict(method, scenes, pair=("vis", "aud"), drop_partner=False):
    """X-C2 / X0-D: co-timed different-cause distractors vs shifted partner."""
    a, b = pair
    stats = {"cotimed_wrong": 0.0, "top1_hit": 0.0, "n": 0, "margin": [],
             "distracted": 0}
    for direction in (0, 1):
        ma, mb = (a, b) if direction == 0 else (b, a)
        for sc in scenes:
            ra, rb = sc.emissions[ma], sc.emissions[mb]
            if len(ra.times) == 0 or len(rb.times) == 0:
                continue
            sim = method.pair_sim(ma, rec(ra), mb, rec(rb))
            partner = sc.partner_index(ma, mb)
            if drop_partner:
                keep = partner >= 0
                sim = sim.copy()
                sim[np.where(keep)[0], partner[keep]] = -np.inf
            fb = false_binding(sim, ra.times, rb.times, ra.events, rb.events,
                               partner)
            stats["cotimed_wrong"] += fb["cotimed_wrong"] * fb["n"]
            stats["top1_hit"] += fb["top1"] * fb["n"]
            stats["n"] += fb["n"]
            stats["distracted"] += fb["n_distracted"]
            if not np.isnan(fb["margin"]):
                stats["margin"].append(fb["margin"])
    n = max(1, stats["n"])
    return {"cotimed_wrong": stats["cotimed_wrong"] / n,
            "top1": stats["top1_hit"] / n,
            "margin": float(np.mean(stats["margin"])) if stats["margin"]
            else float("nan"),
            "n": stats["n"], "n_distracted": stats["distracted"]}


def eval_probes(method, scenes, modalities, n_causes, seed=0):
    """X0-C: per-modality embeddings -> cross-modal probe transfer matrix."""
    embeds, labels = {m: [] for m in modalities}, {m: [] for m in modalities}
    for sc in scenes:
        for m in modalities:
            em = sc.emissions[m]
            if len(em.times) == 0:
                continue
            embeds[m].append(method.embed(m, em.feats))
            labels[m].append(sc.causes[em.events])
    embeds = {m: np.concatenate(v) for m, v in embeds.items() if v}
    labels = {m: np.concatenate(v) for m, v in labels.items() if v}
    return cross_modal_matrix(embeds, labels, n_causes, seed=seed)


def eval_recon(model, scenes, modalities, device="cpu"):
    """X0-B: predict the TRUE partner's features cross-modally -> R^2.
    Uses gt partner index at eval only."""
    kind = getattr(model, "decoders", None)
    ss_err, ss_tot, n = 0.0, 0.0, 0
    per_pair = {}
    with torch.no_grad():
        for sc in scenes:
            for a, b in itertools.permutations(modalities, 2):
                ra, rb = sc.emissions[a], sc.emissions[b]
                partner = sc.partner_index(a, b)
                keep = partner >= 0
                if not keep.any():
                    continue
                xa = torch.as_tensor(ra.feats[keep], dtype=torch.float32,
                                     device=device)
                yb = rb.feats[partner[keep]]
                if hasattr(model, "decode") and f"{a}->{b}" in model.decoders:
                    pred = model.decode(a, b, model.embed(a, xa)).cpu().numpy()
                elif hasattr(model, "decode_into"):
                    z = model.encoders[a](xa)
                    pred = model.decode_into(b, z).cpu().numpy()
                else:
                    continue
                err = ((pred - yb) ** 2).sum()
                ss_err += err
                key = f"{a}->{b}"
                per_pair.setdefault(key, [0.0, 0.0, 0])
                per_pair[key][0] += float(err)
                per_pair[key][1] += float(((yb - yb.mean(0)) ** 2).sum())
                per_pair[key][2] += int(keep.sum())
                ss_tot += ((yb - yb.mean(0)) ** 2).sum()
                n += int(keep.sum())
    r2 = {k: 1.0 - v[0] / max(1e-9, v[1]) for k, v in per_pair.items()}
    return {"r2_macro": float(np.mean(list(r2.values()))) if r2 else float("nan"),
            "per_pair": r2, "n": n}


# ----------------------------------------------------------------------
# full checkpoint evaluation
# ----------------------------------------------------------------------
def build_eval_env(config: Config, seed, alt=False, noise_override=None,
                   **overrides):
    ec = dataclasses.replace(config.env)
    for k, v in overrides.items():
        setattr(ec, k, v)
    return LatentCauseEnv(ec.scene_params(), factors=ec.factors,
                          holdout=ec.holdout, modalities=ec.modalities,
                          seed=seed, alt_transforms=alt,
                          noise_override=noise_override)


def load_model(checkpoint, device="cpu"):
    cp = torch.load(checkpoint, map_location=device, weights_only=False)
    cfg = cp["config"]
    env_cfg = EnvConfig(**{k: (tuple(v) if isinstance(v, list) else v)
                           for k, v in cfg["env"].items()})
    config = Config(seed=cfg["seed"], env=env_cfg,
                    model=ModelSpec(**cfg["model"]))
    env = build_eval_env(config, EVAL_SEED)
    dims = {m: env.transforms[m].out_dim for m in env.modalities}
    model = make_model(config.model, dims).to(device)
    model.load_state_dict(cp["model"])
    model.eval()
    return model, config, cp


def all_methods(model, config, fit_scenes, dims, device="cpu"):
    """Trained model + all baselines, fitted on unlabeled training scenes."""
    methods = {"model": TorchMethod(model, device),
               "timing": TimingMethod(),
               "chance": ChanceMethod(EVAL_SEED),
               "oracle": OracleMethod(),
               "concat": ConcatMethod(config.env.modalities, dims)}
    methods["pca"] = PcaMethod(config.model.d_z).fit(
        {m: np.concatenate([sc.emissions[m].feats for sc in fit_scenes])
         for m in config.env.modalities})
    methods["cca"] = CcaMethod(config.model.d_z).fit(
        fit_scenes, config.env.modalities)
    return methods


def evaluate_checkpoint(checkpoint, artifacts, device="cpu", scenes_n=None,
                        quick=False):
    model, config, cp = load_model(checkpoint, device)
    ecfg = config.eval
    n_scenes = scenes_n or (128 if quick else ecfg.scenes)
    mods = tuple(config.env.modalities)
    env = build_eval_env(config, EVAL_SEED)
    fit_scenes = env.sample_scenes(512, seed=EVAL_SEED + 5)
    methods = all_methods(model, config, fit_scenes,
                          {m: env.transforms[m].out_dim for m in mods}, device)

    out = {"checkpoint": str(checkpoint), "seed": config.seed,
           "kind": config.model.kind, "model_sha256": cp.get("model_sha256"),
           "scenes": n_scenes, "timestamp": time.time()}

    std = env.sample_scenes(n_scenes, seed=EVAL_SEED + 10)
    # ---- X0-A retrieval: all methods ----------------------------------
    out["retrieval"] = {name: eval_retrieval(meth, std, mods)["macro"]
                        for name, meth in methods.items()}

    # ---- X0-B reconstruction (trained model only) ----------------------
    out["recon"] = eval_recon(model, std, mods, device)

    # ---- X0-C probes ----------------------------------------------------
    n_causes = env.causes.n_causes
    probe_methods = {k: v for k, v in methods.items() if k != "oracle"}
    probes = {}
    for name, meth in probe_methods.items():
        try:
            mat = eval_probes(meth, std, mods, n_causes)
        except Exception as e:  # e.g. CCA embed mismatch
            mat = {"error": str(e)}
        probes[name] = mat
    out["probes"] = probes

    # ---- X-C1 desynchronization sweep -----------------------------------
    out["desync"] = {}
    for L in ecfg.lags:
        scs = env.sample_scenes(n_scenes // 2, seed=EVAL_SEED + 100 + L, lag=L)
        out["desync"][str(L)] = {
            name: eval_retrieval(meth, scs, mods)[
                f"{mods[0]}->{mods[1]}"]["top1"]
            for name, meth in methods.items()}

    # ---- X-C2 / X0-D false synchrony (conflict scenes) ------------------
    out["conflict"] = {}
    for d in ecfg.conflict_deltas:
        env_c = build_eval_env(config, EVAL_SEED + 200 + d, conflict_delta=d,
                               modality_drop_p=0.0, token_drop_p=0.0)
        scs = env_c.sample_scenes(n_scenes // 2, seed=0, mode="conflict")
        out["conflict"][str(d)] = {
            name: eval_conflict(meth, scs, pair=tuple(config.env.conflict_pair))
            for name, meth in methods.items()}
        # partner-absent rejection: true partner removed from candidates
        out["conflict"][str(d)]["_absent"] = {
            name: eval_conflict(meth, scs,
                                pair=tuple(config.env.conflict_pair),
                                drop_partner=True)
            for name, meth in methods.items()}

    # ---- X-C3 dropout ----------------------------------------------------
    out["dropout"] = {"token": {}, "modality_absent": {}}
    for pdrop in ecfg.drop_rates:
        scs = env.sample_scenes(n_scenes // 2, seed=EVAL_SEED + 300,
                                token_drop_p=pdrop, modality_drop_p=0.0)
        out["dropout"]["token"][str(pdrop)] = {
            name: eval_retrieval(meth, scs, mods)["macro"]["top1"]
            for name, meth in methods.items()}
    # whole modality removed (aud) -> retrieval over remaining pairs
    scs = env.sample_scenes(n_scenes // 2, seed=EVAL_SEED + 301,
                            token_drop_p=0.0, modality_drop_p=0.0)
    missing = "aud"
    rem = tuple(m for m in mods if m != missing)
    for sc in scs:
        sc.emissions.pop(missing, None)
    out["dropout"]["modality_absent"] = {
        name: eval_retrieval(meth, scs, rem)["macro"]
        for name, meth in methods.items()}

    # ---- X-C4 sensor transform ------------------------------------------
    env_alt = build_eval_env(config, EVAL_SEED + 400, alt=True)
    scs_alt = env_alt.sample_scenes(n_scenes // 2, seed=0)
    out["transform"] = {
        name: eval_retrieval(meth, scs_alt, mods)["macro"]
        for name, meth in methods.items()}

    # ---- X-C5 recombination ----------------------------------------------
    scs_h = env.sample_scenes(n_scenes // 2, seed=EVAL_SEED + 500,
                              cause_pool="holdout")
    out["recombination"] = {
        name: eval_retrieval(meth, scs_h, mods)["macro"]
        for name, meth in methods.items()}

    # ---- latent visualisation data (PCA of model embeddings) -------------
    out["_latent"] = _latent_snapshot(model, std[:128], mods, device)

    dest = Path(artifacts) / "eval" / (Path(checkpoint).stem + ".json")
    atomic(dest, {k: v for k, v in out.items() if not k.startswith("_")})
    return out


def _latent_snapshot(model, scenes, modalities, device):
    embeds, causes, mods_l = [], [], []
    with torch.no_grad():
        for sc in scenes:
            for m in modalities:
                em = sc.emissions[m]
                if len(em.times) == 0:
                    continue
                x = torch.as_tensor(em.feats, dtype=torch.float32,
                                    device=device)
                embeds.append(model.embed(m, x).cpu().numpy())
                causes.append(sc.causes[em.events])
                mods_l += [m] * len(em.times)
    if not embeds:
        return None
    Z = np.concatenate(embeds)
    c = np.concatenate(causes)
    Zc = Z - Z.mean(0)
    _, _, vt = np.linalg.svd(Zc, full_matrices=False)
    proj = Zc @ vt[:3].T
    return {"proj": proj.tolist(), "cause": c.tolist(), "mod": mods_l}


# ----------------------------------------------------------------------
# pre-learning environment validation (oracle/random/timing sanity table)
# ----------------------------------------------------------------------
def env_validation(config: Config, n_scenes=256, seed=1234):
    env = build_eval_env(config, seed)
    mods = tuple(config.env.modalities)
    methods = {"oracle": OracleMethod(), "timing": TimingMethod(),
               "chance": ChanceMethod(seed)}
    rows = {}
    std = env.sample_scenes(n_scenes, seed=seed + 1)
    rows["standard"] = {nm: eval_retrieval(me, std, mods)["macro"]
                        for nm, me in methods.items()}
    # shifted-timing control: all pairs shifted by a fixed lag
    env_sh = build_eval_env(config, seed, fixed_shift=3)
    shifted = env_sh.sample_scenes(n_scenes, seed=seed + 2)
    rows["shifted_lag3"] = {nm: eval_retrieval(me, shifted, mods)["macro"]
                           for nm, me in methods.items()}
    # false-synchrony control
    env_c = build_eval_env(config, seed, conflict_delta=2,
                           token_drop_p=0.0, modality_drop_p=0.0)
    conf = env_c.sample_scenes(n_scenes, seed=seed + 3, mode="conflict")
    rows["conflict"] = {nm: eval_conflict(
        me, conf, pair=tuple(config.env.conflict_pair))
        for nm, me in methods.items()}
    return rows


# ----------------------------------------------------------------------
# consolidation across seeds
# ----------------------------------------------------------------------
def consolidate(artifacts):
    eval_dir = Path(artifacts) / "eval"
    rows = [json.loads(p.read_text()) for p in sorted(eval_dir.glob("*.json"))
            if p.name != "consolidated.json"]
    by_kind = {}
    for r in rows:
        by_kind.setdefault(r["kind"], []).append(r)
    summary = {}
    for kind, items in by_kind.items():
        s = {"seeds": [r["seed"] for r in items]}
        s["retrieval_top1"] = summarise(
            [r["retrieval"]["model"]["top1"] for r in items])
        s["retrieval_match"] = summarise(
            [r["retrieval"]["model"]["match"] for r in items])
        s["recon_r2"] = summarise([r["recon"]["r2_macro"] for r in items])
        # cross-modal probe transfer: mean off-diagonal of model probe matrix
        cross_probe = []
        for r in items:
            mat = r["probes"].get("model")
            if isinstance(mat, dict) and not mat.get("error"):
                off = [v for a, b in itertools.product(mat, mat)
                       for k2, v in [(b, mat[a].get(b))] if a != b
                       and v is not None and not np.isnan(v)]
                # simpler: off-diagonal entries
                off = [mat[a][b] for a in mat for b in mat[a] if a != b]
                cross_probe.append(float(np.nanmean(off)))
        s["probe_cross_modal"] = summarise(cross_probe)
        s["probe_within_modal"] = summarise([
            float(np.nanmean([r["probes"]["model"][a][a] for a in
                              r["probes"]["model"]]))
            for r in items if isinstance(r["probes"].get("model"), dict)])
        for L in items[0]["desync"]:
            s[f"desync_{L}"] = summarise(
                [r["desync"][L]["model"] for r in items])
        for d in items[0]["conflict"]:
            s[f"conflict_{d}_fbr"] = summarise(
                [r["conflict"][d]["model"]["cotimed_wrong"] for r in items])
            s[f"conflict_{d}_top1"] = summarise(
                [r["conflict"][d]["model"]["top1"] for r in items])
        s["dropout_absent"] = summarise(
            [r["dropout"]["modality_absent"]["model"]["top1"] for r in items])
        s["transform"] = summarise(
            [r["transform"]["model"]["top1"] for r in items])
        s["recombination"] = summarise(
            [r["recombination"]["model"]["top1"] for r in items])
        # effect size vs timing baseline (standard top1, conflict fbr)
        timing_top1 = [r["retrieval"]["timing"]["top1"] for r in items]
        model_top1 = [r["retrieval"]["model"]["top1"] for r in items]
        s["vs_timing"] = {"cohens_d": cohens_d(model_top1, timing_top1),
                          "sign": sign_test_p(model_top1, timing_top1)}
        timing_fbr = [r["conflict"][list(items[0]["conflict"])[0]]["timing"]
                      ["cotimed_wrong"] for r in items]
        model_fbr = [r["conflict"][list(items[0]["conflict"])[0]]["model"]
                     ["cotimed_wrong"] for r in items]
        s["fbr_vs_timing"] = {"cohens_d": cohens_d(model_fbr, timing_fbr),
                              "sign": sign_test_p(model_fbr, timing_fbr)}
        summary[kind] = s
    atomic(Path(artifacts) / "eval" / "consolidated.json", summary)
    return summary


def make_plots(artifacts):
    eval_dir = Path(artifacts) / "eval"
    figdir = Path(artifacts) / "eval" / "figs"
    figdir.mkdir(parents=True, exist_ok=True)
    rows = [json.loads(p.read_text()) for p in sorted(eval_dir.glob("*.json"))
            if p.name != "consolidated.json"]
    if rows:
        plot_retrieval_bars(rows, figdir / "retrieval_bars.png")
        plot_false_binding(rows, figdir / "false_binding.png")
        plot_lag_curve(rows, figdir / "desync_curve.png")
        plot_probe_heatmap(rows[0]["probes"]["model"],
                           figdir / "probe_transfer.png")
    return figdir


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--checkpoint")
    p.add_argument("--artifacts", required=True)
    p.add_argument("--scenes", type=int, default=None)
    p.add_argument("--device", default="cpu")
    p.add_argument("--quick", action="store_true")
    p.add_argument("--consolidate-only", action="store_true")
    p.add_argument("--env-validation", action="store_true")
    p.add_argument("--base-config", default=None,
                   help="yaml config for --env-validation")
    a = p.parse_args()
    if a.env_validation:
        from experiments.x0.config import load_config
        cfg = load_config(a.base_config) if a.base_config else Config()
        rows = env_validation(cfg)
        dest = Path(a.artifacts) / "env_validation.json"
        atomic(dest, rows)
        print(json.dumps(rows, indent=2))
        return
    if a.consolidate_only:
        s = consolidate(a.artifacts)
        make_plots(a.artifacts)
        print(json.dumps(s, indent=2))
        return
    out = evaluate_checkpoint(a.checkpoint, a.artifacts, a.device,
                              a.scenes, a.quick)
    print(json.dumps({k: v for k, v in out.items() if not k.startswith("_")},
                     indent=2))


if __name__ == "__main__":
    main()
