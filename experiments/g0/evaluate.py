"""G0 evaluation battery.

For a representation (trained model or analytic baseline), computes:

  probes        linear cause probes: in-context, leave-one-context-out,
                held-out permutation context, held-out dense context,
                few-shot, best-action transfer, boundary detection,
                intensity regression
  clustering    k-means vs ground-truth causes: NMI, purity (pooled and
                per-context), on held-out context
  matching      same-cause/different-appearance: per-cause latent
                centroids matched across contexts (nearest-centroid
                accuracy + cosine margin)
  causal        time-shuffle sensitivity, sensor dropout, mid-episode
                context switch, decoy confusion (COLD<->HEAT)
  ood           unseen context, unseen dense context, novel cause
                composition (held-out pairs), noise shift, gain shift
  discrete      (VQ/kmeans reps) code usage, code<->cause MI, stability
  intervention  decode-delta directions vs true per-context cause
                prototypes (selectivity = diag - max off-diag cos)

Usage:
    python -m experiments.g0.evaluate --run-dir runs/gru__seed0
    python -m experiments.g0.evaluate --rep raw --seed 0
"""

from __future__ import annotations

import argparse
import dataclasses
import json
from pathlib import Path

import numpy as np
import torch

from .analysis.metrics import (cause_centroids, dropout_dims,
                               episode_split, flat_latents, flat_steps,
                               obs_space_prototypes, shuffle_episode_steps)
from .config import Config, load_config
from .data import EVAL_SEED_OFFSET, collect_dataset
from .env import LatentCauseEnv, N_CAUSES
from .env.dynamics import NEUTRAL
from .models import build_model
from .probes import (FEAT_NAMES, best_match_acc, centroid_margin,
                     dynseg_dataset, fewshot_probe, kmeans,
                     logistic_probe, mutual_info, nmi, purity,
                     ridge_probe)
from .representations import (KMeansRep, TorchRep, build_analytic_rep,
                              build_kmeans_rep)
from .train import load_ckpt

REPORTS = Path(__file__).parent / "reports"


def _enc(rep, ds):
    return rep.encode(ds["obs"], ds["actions"])


def _acc(X, y, tr, te, n_classes, ecfg, seed, ret_full=False):
    r = logistic_probe(X[tr], y[tr], X[te], y[te], n_classes=n_classes,
                       steps=ecfg.probe_steps, lr=ecfg.probe_lr, seed=seed)
    return r if ret_full else r["acc"]


def _eval_datasets(cfg: Config, seed: int, env_seed: int):
    """All evaluation datasets. ctx_ids assigned round-robin for
    deterministic context coverage."""
    ec = cfg.env
    ood = cfg.eval.ood
    s = seed + EVAL_SEED_OFFSET
    n_ep = cfg.eval.episodes
    train_ctx = list(range(ec.n_train_contexts))
    out = {}
    out["main"] = collect_dataset(ec, n_ep, s, cfg.data.policy,
                                  env_seed=env_seed, ctx_ids=train_ctx)
    out["ood_ctx"] = collect_dataset(ec, n_ep, s + 100, cfg.data.policy,
                                   env_seed=env_seed,
                                   ctx_ids=[ood.ood_ctx_id])
    out["dense_ctx"] = collect_dataset(ec, n_ep, s + 200, cfg.data.policy,
                                     env_seed=env_seed,
                                     ctx_ids=[ood.ood_dense_ctx_id])
    ec_combo = dataclasses.replace(ec, pair_prob=ood.pair_prob)
    out["combo"] = collect_dataset(ec_combo, n_ep, s + 300,
                                   cfg.data.policy, env_seed=env_seed,
                                   ctx_ids=train_ctx, pair_set="ood")
    ec_noise = dataclasses.replace(ec, obs_noise=ec.obs_noise
                                   * ood.noise_mul)  # read-noise shift
    out["noise"] = collect_dataset(ec_noise, n_ep, s + 400,
                                   cfg.data.policy, env_seed=env_seed,
                                   ctx_ids=train_ctx)
    env_gain = LatentCauseEnv(ec, seed=env_seed,
                              rng_seed=(s + 500) * 1_000_033 + 811)
    env_gain.set_action_gain(ood.action_gain_mul)
    out["gain"] = collect_dataset(ec, n_ep, s + 500, cfg.data.policy,
                                  env=env_gain, ctx_ids=train_ctx)
    half = ec.episode_len // 2
    out["midctx"] = collect_dataset(
        ec, n_ep, s + 600, cfg.data.policy, env_seed=env_seed,
        ctx_ids=[0], ctx_schedule={half: ood.ood_ctx_id})
    # true compositional-OOD: held-out cause pairs rendered in the
    # held-out context (within-ctx combo presence can be answered by
    # per-cause appearance detectors; this cannot)
    out["combo_oodctx"] = collect_dataset(
        ec_combo, n_ep, s + 700, cfg.data.policy, env_seed=env_seed,
        ctx_ids=[ood.ood_ctx_id], pair_set="ood")
    return out


def eval_representation(rep, cfg: Config, seed: int, device: str = "cpu",
                        model=None, train_ds=None,
                        env_seed=None, dss=None) -> dict:
    ec = cfg.env
    ev = cfg.eval
    ood = ev.ood
    warm = ev.warmup
    env_seed = seed if env_seed is None else env_seed
    if dss is None:
        dss = _eval_datasets(cfg, seed, env_seed)

    out = {"rep": rep.name, "seed": seed, "env_seed": env_seed,
           "latent_dim": rep.latent_dim}

    # ---- encode ------------------------------------------------------
    Z = {k: _enc(rep, d) for k, d in dss.items()}
    lab = {k: flat_steps(d, warm) for k, d in dss.items()}
    X = {k: flat_latents(Z[k], warm) for k, d in dss.items()}

    main = lab["main"]
    # cause-id probes exclude pair steps AND the first seg_warmup steps
    # of each segment (no evidence yet)
    single = main["single"] & (main["seg_pos"] >= ev.seg_warmup)
    y = main["cause_a"]
    E = dss["main"]["actions"].shape[0]
    tr_ep, te_ep = episode_split(E)
    tr = single & np.isin(main["episode"], np.where(tr_ep)[0])
    te = single & np.isin(main["episode"], np.where(te_ep)[0])

    # ---- probes -------------------------------------------------------
    p = {}
    p["acc_in"] = _acc(X["main"], y, tr, te, N_CAUSES, ev, seed)

    # leave-one-context-out on training contexts
    loco = {}
    for j in range(ec.n_train_contexts):
        mte = single & (main["ctx"] == j)
        mtr = single & (main["ctx"] != j)
        if mte.sum() < N_CAUSES or mtr.sum() < 50:
            continue
        loco[str(j)] = _acc(X["main"], y, mtr, mte, N_CAUSES, ev, seed)
    p["loco"] = loco
    p["acc_loco"] = float(np.mean(list(loco.values()))) if loco else float("nan")

    # held-out contexts: probe trained on all train ctxs
    full_tr = single
    from .analysis.metrics import ep_demean
    grp_main = main["episode"] * 64 + main["ctx"]
    Xm_dm = ep_demean(X["main"], grp_main)
    for key, tag in (("ood_ctx", "acc_ood_ctx"),
                     ("dense_ctx", "acc_dense_ctx")):
        mte = lab[key]["single"] & (lab[key]["seg_pos"] >= ev.seg_warmup)
        r = logistic_probe(X["main"][full_tr], y[full_tr],
                           X[key][mte], lab[key]["cause_a"][mte],
                           n_classes=N_CAUSES, steps=ev.probe_steps,
                           lr=ev.probe_lr, seed=seed)
        p[tag] = r["acc"]
        # episode-demeaned variant: strips the per-episode/context
        # nuisance block before probing
        Xk_dm = ep_demean(X[key], lab[key]["episode"] * 64
                          + lab[key]["ctx"])
        r2 = logistic_probe(Xm_dm[full_tr], y[full_tr], Xk_dm[mte],
                            lab[key]["cause_a"][mte], n_classes=N_CAUSES,
                            steps=ev.probe_steps, lr=ev.probe_lr,
                            seed=seed)
        p[tag + "_dm"] = r2["acc"]
        if key == "ood_ctx":
            p["ood_confusion"] = r["confusion"]
            p["ood_acc_per_class"] = r["acc_per_class"]

    # few-shot (within pooled train ctxs, on test half)
    fs = {}
    for m in ev.fewshot:
        r = fewshot_probe(X["main"][te], y[te], m, N_CAUSES, seed=seed,
                          steps=ev.probe_steps, lr=ev.probe_lr)
        fs[str(m)] = r["acc"]
    p["fewshot_in"] = fs
    # few-shot in the held-out context (adaptation with m labels)
    fs_ood = {}
    mo = lab["ood_ctx"]["single"] & (lab["ood_ctx"]["seg_pos"]
                                     >= ev.seg_warmup)
    for m in ev.fewshot:
        r = fewshot_probe(X["ood_ctx"][mo], lab["ood_ctx"]["cause_a"][mo],
                          m, N_CAUSES, seed=seed, steps=ev.probe_steps,
                          lr=ev.probe_lr)
        fs_ood[str(m)] = r["acc"]
    p["fewshot_ood"] = fs_ood

    # novel-task transfer: best-action class + boundary detection + x
    yb = main["best_action"]
    p["best_action_acc"] = _acc(X["main"], yb, tr, te, N_ACTIONS_, ev, seed)
    # the label is imbalanced (TAP is the argmax drive for 3/6 causes):
    # majority-vote is the honest floor, not 1/4
    maj_cls = int(np.bincount(yb[tr], minlength=N_ACTIONS_).argmax())
    p["best_action_maj_baseline"] = float((yb[te] == maj_cls).mean())
    p["best_action_delta"] = p["best_action_acc"] \
        - p["best_action_maj_baseline"]
    r = logistic_probe(X["main"][tr], main["switch_next"][tr].astype(int),
                       X["main"][te], main["switch_next"][te].astype(int),
                       n_classes=2, steps=ev.probe_steps, lr=ev.probe_lr,
                       seed=seed)
    p["boundary_acc"] = r["acc"]
    p["boundary_auc"] = r.get("auc", float("nan"))
    rr = ridge_probe(X["main"][tr & single],
                     main["x_a"][tr & single, None],
                     X["main"][te & single],
                     main["x_a"][te & single, None])
    p["x_r2"] = rr["r2"][0]

    # ---- segment-level ("concept") probes: pool each single-cause
    # segment's latents, classify the segment's cause -------------------
    # two poolings: mean (linear-ish) and dynamical-signature features
    # (nonlinear; answers "the linear probe lacked power" objections)
    from .analysis.metrics import segment_means
    seg = {k: segment_means(Z[k], dss[k], warm)
           for k in ("main", "ood_ctx", "dense_ctx")}
    p.update(_seg_probes(seg, ec.n_train_contexts, ev, seed, "seg_"))
    # next-step latents: pairs each delta with the action that caused it
    # (for instantaneous reps, diff-based deltas would misalign by one)
    Zn = {k: _enc(rep, {"obs": dss[k]["next_obs"], "actions": dss[k]["actions"]})
          for k in ("main", "ood_ctx", "dense_ctx")}
    dseg = {k: dynseg_dataset(Z[k], dss[k]["actions"], dss[k], warm,
                              next_X=Zn[k])
            for k in ("main", "ood_ctx", "dense_ctx")}
    p.update(_seg_probes(dseg, ec.n_train_contexts, ev, seed, "dynseg_"))
    out["probes"] = p

    # ---- clustering ----------------------------------------------------
    cl = {}
    from .probes import nmi as nmi_fn, purity as pur_fn
    labs_single = y[single]
    _, assign = kmeans(X["main"][single], N_CAUSES, seed=seed)
    cl["nmi_pooled"] = nmi_fn(labs_single, assign)
    cl["purity_pooled"] = pur_fn(labs_single, assign)
    per_ctx = {}
    for j in range(ec.n_train_contexts):
        m = single & (main["ctx"] == j)
        if m.sum() < N_CAUSES * 3:
            continue
        _, a = kmeans(X["main"][m], N_CAUSES, seed=seed)
        per_ctx[str(j)] = nmi_fn(y[m], a)
    cl["nmi_per_ctx"] = per_ctx
    cl["nmi_worst_ctx"] = min(per_ctx.values()) if per_ctx else float("nan")
    mo = lab["ood_ctx"]["single"] & (lab["ood_ctx"]["seg_pos"]
                                     >= ev.seg_warmup)
    _, a = kmeans(X["ood_ctx"][mo], N_CAUSES, seed=seed)
    cl["nmi_ood_ctx"] = nmi_fn(lab["ood_ctx"]["cause_a"][mo], a)
    out["clustering"] = cl

    # ---- same cause / different appearance: cross-context matching -----
    ctx_ids_all = list(range(ec.n_train_contexts)) + [ood.ood_ctx_id,
                                                      ood.ood_dense_ctx_id]

    def _key(j):
        return "main" if j < ec.n_train_contexts else (
            "ood_ctx" if j == ood.ood_ctx_id else "dense_ctx")

    cents, cents_dm = {}, {}
    for j in ctx_ids_all:
        key = _key(j)
        # -1 labels on pair steps AND the first seg_warmup steps keep
        # them out of the centroids (same masking as the step probes)
        lab_single = np.where(
            lab[key]["single"] & (lab[key]["seg_pos"] >= ev.seg_warmup),
            lab[key]["cause_a"], -1)
        cc, ids = cause_centroids(X[key], lab_single,
                                  lab[key]["ctx"], j)
        cents[j] = (cc, ids)
        grp = lab[key]["episode"] * 64 + lab[key]["ctx"]
        Xdm = ep_demean(X[key], grp)
        cc2, ids2 = cause_centroids(Xdm, lab_single,
                                    lab[key]["ctx"], j)
        cents_dm[j] = (cc2, ids2)

    match_pairs, match_pairs_dm = {}, {}
    for i in ctx_ids_all:
        for j in ctx_ids_all:
            if j <= i:
                continue
            ca, ia = cents[i]
            cb, ib = cents[j]
            m = best_match_acc(ca, ia, cb, ib)
            mg = centroid_margin(ca, ia, cb, ib)
            match_pairs[f"{i}-{j}"] = {**m, **mg}
            ca, ia = cents_dm[i]
            cb, ib = cents_dm[j]
            m = best_match_acc(ca, ia, cb, ib)
            mg = centroid_margin(ca, ia, cb, ib)
            match_pairs_dm[f"{i}-{j}"] = {**m, **mg}

    def _agg(pairs, train_only):
        sel = [v for k, v in pairs.items()
               if all(int(t) < ec.n_train_contexts for t in k.split("-"))
               == train_only]
        mt = [v["match_acc"] for v in sel
              if not np.isnan(v["match_acc"])]
        mg = [v["margin"] for v in sel
              if not np.isnan(v.get("margin", np.nan))]
        return (float(np.mean(mt)) if mt else float("nan"),
                float(np.mean(mg)) if mg else float("nan"))

    # label-shuffle null: permute the cause->centroid assignment on one
    # side — calibrates match_ood against its empirical chance level
    rngm = np.random.default_rng(seed + 37)
    match_pairs_null = {}
    for i in ctx_ids_all:
        for j in ctx_ids_all:
            if j <= i:
                continue
            ca, ia = cents[i]
            cb, ib = cents[j]
            if len(cb) >= 2:
                permj = rngm.permutation(len(cb))
                mn = best_match_acc(ca, ia, cb[permj], ib)
                match_pairs_null[f"{i}-{j}"] = mn["match_acc"]
    mt_null, _ = _agg({k: {"match_acc": v} for k, v in
                       match_pairs_null.items()}, False)

    mt_raw, mg_train = _agg(match_pairs, True)
    mo_raw, mg_ood = _agg(match_pairs, False)
    mt_dm, _ = _agg(match_pairs_dm, True)
    mo_dm, mg_ood_dm = _agg(match_pairs_dm, False)
    out["matching"] = {
        "pairs": match_pairs,
        "match_train_ctx": mt_raw,
        "match_ood_ctx": mo_raw,
        "match_ood_ctx_null": mt_null,
        "margin_ood_ctx": mg_ood,
        "match_train_ctx_dm": mt_dm,
        "match_ood_ctx_dm": mo_dm,
        "margin_ood_ctx_dm": mg_ood_dm,
        "pairs_dm": match_pairs_dm,
    }

    # ---- causal tests --------------------------------------------------
    cz = {}
    o_sh, a_sh = shuffle_episode_steps(dss["main"]["obs"],
                                       dss["main"]["actions"], seed)
    X_sh = flat_latents(rep.encode(o_sh, a_sh), warm)
    p_sh = logistic_probe(X_sh[tr], y[tr], X_sh[te], y[te],
                          n_classes=N_CAUSES, steps=ev.probe_steps,
                          lr=ev.probe_lr, seed=seed)
    cz["shuffle_acc"] = p_sh["acc"]
    cz["shuffle_delta"] = p["acc_in"] - p_sh["acc"]
    _, a_sh_cl = kmeans(X_sh[single], N_CAUSES, seed=seed)
    cz["shuffle_nmi"] = nmi_fn(labs_single, a_sh_cl)

    # label-aligned shuffle: obs/action/labels permuted together, so only
    # temporal order is destroyed. Isolates how much of the rep's cause
    # decodability is instantaneous appearance vs dynamical context —
    # the "sensor shortcut" check.
    o_sh2, a_sh2, perms = shuffle_episode_steps(
        dss["main"]["obs"], dss["main"]["actions"], seed + 5,
        ret_perm=True)
    ca_full = dss["main"]["cause_a"]
    cb_full = dss["main"]["cause_b"]
    Ef = ca_full.shape[0]
    y_al = np.stack([ca_full[e][perms[e]] for e in range(Ef)])
    cb_al = np.stack([cb_full[e][perms[e]] for e in range(Ef)])
    y_al = y_al[:, warm:].ravel()
    single_al = cb_al[:, warm:].ravel() < 0
    tr_al = single_al & np.isin(main["episode"], np.where(tr_ep)[0])
    te_al = single_al & np.isin(main["episode"], np.where(te_ep)[0])
    X_sh2 = flat_latents(rep.encode(o_sh2, a_sh2), warm)
    r = logistic_probe(X_sh2[tr_al], y_al[tr_al], X_sh2[te_al],
                       y_al[te_al], n_classes=N_CAUSES,
                       steps=ev.probe_steps, lr=ev.probe_lr, seed=seed)
    cz["shuffle_aligned_acc"] = r["acc"]
    cz["shuffle_aligned_delta"] = p["acc_in"] - r["acc"]

    # sensor permutation: scramble obs dims with a fixed permutation and
    # probe with a decoder trained on unpermuted data. Equivalent to a
    # hard context change with no shared channel ordering.
    rngp = np.random.default_rng(seed + 9)
    perm_dims = rngp.permutation(dss["main"]["obs"].shape[-1])
    o_perm = dss["main"]["obs"][..., perm_dims]
    X_perm = flat_latents(rep.encode(o_perm, dss["main"]["actions"]),
                          warm)
    r = logistic_probe(X["main"][tr], y[tr], X_perm[te], y[te],
                       n_classes=N_CAUSES, steps=ev.probe_steps,
                       lr=ev.probe_lr, seed=seed)
    cz["perm_acc"] = r["acc"]
    cz["perm_delta"] = p["acc_in"] - r["acc"]

    o_drop = dropout_dims(dss["main"]["obs"], ood.dropout_frac, seed)
    X_drop = flat_latents(rep.encode(o_drop, dss["main"]["actions"]), warm)
    r = logistic_probe(X["main"][tr], y[tr], X_drop[te], y[te],
                       n_classes=N_CAUSES, steps=ev.probe_steps,
                       lr=ev.probe_lr, seed=seed)
    cz["dropout_acc"] = r["acc"]

    # mid-episode context switch: train on pre-switch (ctx 0), test on
    # post-switch steps (held-out ctx), same episode => same cause stream
    half = ec.episode_len // 2 - warm
    midm = lab["midctx"]["single"] & (lab["midctx"]["seg_pos"]
                                      >= ev.seg_warmup)
    sop = lab["midctx"]["step_of_ep"]
    pre = midm & (sop < half - 1)
    post = midm & (sop > half + 1)
    r = logistic_probe(X["midctx"][pre], lab["midctx"]["cause_a"][pre],
                       X["midctx"][post], lab["midctx"]["cause_a"][post],
                       n_classes=N_CAUSES, steps=ev.probe_steps,
                       lr=ev.probe_lr, seed=seed)
    cz["midctx_acc"] = r["acc"]

    # carryover decay: probe acc at increasing lag after the ctx switch
    lag = sop - half  # steps since the context switch
    cz["midctx_by_lag"] = {}
    for lo, hi in ((1, 4), (5, 9), (10, 16), (17, 31)):
        m = midm & (lag >= lo) & (lag <= hi)
        if m.sum() < 20:
            continue
        r = logistic_probe(X["midctx"][pre],
                           lab["midctx"]["cause_a"][pre],
                           X["midctx"][m], lab["midctx"]["cause_a"][m],
                           n_classes=N_CAUSES, steps=ev.probe_steps,
                           lr=ev.probe_lr, seed=seed)
        cz["midctx_by_lag"][f"{lo}-{hi}"] = r["acc"]

    # label-shuffle null for midctx: permute post-switch cause labels
    # within each episode — empirical floor for "above chance" claims
    rngn = np.random.default_rng(seed + 31)
    post_lab_full = lab["midctx"]["cause_a"]
    ep_m = lab["midctx"]["episode"]
    post_idx = np.where(post)[0]
    nulls = []
    for _ in range(3):
        pl = post_lab_full.copy()
        for e in np.unique(ep_m[post_idx]):
            m = post_idx[ep_m[post_idx] == e]
            pl[m] = pl[m][rngn.permutation(len(m))]
        rn = logistic_probe(X["midctx"][pre], post_lab_full[pre],
                            X["midctx"][post], pl[post],
                            n_classes=N_CAUSES, steps=ev.probe_steps,
                            lr=ev.probe_lr, seed=seed)
        nulls.append(rn["acc"])
    cz["midctx_acc_null"] = float(np.mean(nulls))

    # decoy: most-confusable pair under held-out context (expected:
    # HEAT<->COLD) from the ood confusion matrix
    conf = np.asarray(p["ood_confusion"])
    off = conf.copy()
    np.fill_diagonal(off, 0.0)
    decoy = np.unravel_index(off.argmax(), off.shape)
    cz["decoy_pair"] = [int(decoy[0]), int(decoy[1])]
    cz["decoy_rate"] = float(off[decoy])
    out["causal"] = cz

    # ---- OOD -----------------------------------------------------------
    od = {"acc_ood_ctx": p["acc_ood_ctx"],
          "acc_dense_ctx": p["acc_dense_ctx"]}
    # noise shift: probe trained on clean, tested on noisy
    mn = lab["noise"]["single"] & (lab["noise"]["seg_pos"]
                                   >= ev.seg_warmup)
    r = logistic_probe(X["main"][full_tr], y[full_tr], X["noise"][mn],
                       lab["noise"]["cause_a"][mn], n_classes=N_CAUSES,
                       steps=ev.probe_steps, lr=ev.probe_lr, seed=seed)
    od["acc_noise"] = r["acc"]
    # gain shift (intensity range beyond training)
    mg = lab["gain"]["single"] & (lab["gain"]["seg_pos"] >= ev.seg_warmup)
    r = logistic_probe(X["main"][full_tr], y[full_tr], X["gain"][mg],
                       lab["gain"]["cause_a"][mg], n_classes=N_CAUSES,
                       steps=ev.probe_steps, lr=ev.probe_lr, seed=seed)
    od["acc_gain"] = r["acc"]

    # novel composition: presence AUC per non-neutral cause on
    # held-out pairs (probes trained on train-pair data)
    pres_tr = np.zeros((len(y), N_CAUSES))
    for c in range(N_CAUSES):
        pres_tr[:, c] = (y == c) | (main["cause_b"] == c)
    cb_lab = lab["combo"]
    pres_te = np.zeros((len(cb_lab["cause_a"]), N_CAUSES))
    for c in range(N_CAUSES):
        pres_te[:, c] = ((cb_lab["cause_a"] == c)
                         | (cb_lab["cause_b"] == c))
    aucs, set_hits = [], []
    probs = np.zeros((len(pres_te), N_CAUSES))
    for c in range(1, N_CAUSES):  # skip NEUTRAL
        if pres_tr[:, c].sum() < 10:
            continue
        r = logistic_probe(X["main"], pres_tr[:, c].astype(int),
                           X["combo"], pres_te[:, c].astype(int),
                           n_classes=2, steps=ev.probe_steps,
                           lr=ev.probe_lr, seed=seed)
        aucs.append(r.get("auc", float("nan")))
        probs[:, c] = r.get("prob_pos", np.zeros(len(pres_te)))
    od["combo_presence_auc"] = float(np.nanmean(aucs)) if aucs else float("nan")
    pair_steps = cb_lab["cause_b"] >= 0
    if pair_steps.sum() > 0:
        top2 = np.argsort(-probs[pair_steps], axis=1)[:, :2]
        true_sets = np.stack([cb_lab["cause_a"][pair_steps],
                              cb_lab["cause_b"][pair_steps]], axis=1)
        hit = [set(t2) == set(ts) for t2, ts in
               zip(top2, true_sets.tolist())]
        od["combo_set_acc"] = float(np.mean(hit))
    else:
        od["combo_set_acc"] = float("nan")

    # held-out pairs rendered in the HELD-OUT context: within-context
    # presence probing can be answered by per-cause appearance
    # detectors; this version additionally requires appearance transfer
    cb2 = lab["combo_oodctx"]
    pres_te2 = np.zeros((len(cb2["cause_a"]), N_CAUSES))
    for c in range(N_CAUSES):
        pres_te2[:, c] = ((cb2["cause_a"] == c) | (cb2["cause_b"] == c))
    aucs2 = []
    for c in range(1, N_CAUSES):
        if pres_tr[:, c].sum() < 10:
            continue
        r = logistic_probe(X["main"], pres_tr[:, c].astype(int),
                           X["combo_oodctx"], pres_te2[:, c].astype(int),
                           n_classes=2, steps=ev.probe_steps,
                           lr=ev.probe_lr, seed=seed)
        aucs2.append(r.get("auc", float("nan")))
    od["combo_oodctx_auc"] = float(np.nanmean(aucs2)) \
        if aucs2 else float("nan")
    out["ood"] = od

    # ---- discrete-code analysis ---------------------------------------
    codes = rep.codes(dss["main"]["obs"], dss["main"]["actions"])
    if codes is not None:
        cf = codes[:, warm:].ravel()
        csingle = cf[single]
        used = np.unique(csingle)
        mi = mutual_info(csingle, labs_single)
        h_c = mutual_info(labs_single, labs_single)
        # code stability within segments (consecutive steps, same segment)
        seg = main["seg_id"][single]
        same_seg = seg[1:] == seg[:-1]
        stab = float(np.mean(csingle[1:][same_seg] == csingle[:-1][same_seg]))
        if isinstance(rep, KMeansRep):
            cb_size = rep.k
        elif hasattr(rep, "model") and hasattr(rep.model, "vq"):
            cb_size = int(rep.model.vq.n_codes)
        else:
            cb_size = rep.latent_dim
        out["discrete"] = {
            "n_codes_used": int(len(used)),
            "mi_code_cause": float(mi),
            "mi_code_cause_norm": float(mi / max(h_c, EPS)),
            "code_stability": stab,
            "codebook_size": cb_size,
        }
    else:
        out["discrete"] = None

    # ---- latent intervention (decode-delta vs cause prototypes) -------
    # only when the rep's latent feeds the model head/decoder directly
    # (skip post-hoc reps like k-means whose dim mismatches)
    exp_dim = None
    if model is not None:
        if hasattr(model, "head"):
            exp_dim = int(model.head.in_features)
        elif hasattr(model, "decoder"):
            exp_dim = int(model.decoder[0].in_features)
    if model is not None and exp_dim == rep.latent_dim:
        out["intervention"] = _intervention(model, rep, X, lab, cfg,
                                            env_seed)
    elif model is not None:
        out["intervention"] = {}
    # ---- base loss for trained models ----------------------------------
    if model is not None:
        out["base_mse"] = _base_mse(model, dss["main"], device)
    return out


def _seg_probes(seg: dict, n_train_ctx: int, ev, seed: int,
                prefix: str) -> dict:
    """Shared segment-level probe block. `seg` maps dataset key ->
    {"Z","cause","ctx","ep"} as produced by segment_means or
    dynseg_dataset. Emits <prefix>acc_in / loco / acc_ood_ctx /
    acc_dense_ctx."""
    out = {}
    sX, sy = seg["main"]["Z"], seg["main"]["cause"]
    sctx, sep = seg["main"]["ctx"], seg["main"]["ep"]
    if len(sy) < 40:
        return out
    s_tr_ep, s_te_ep = episode_split(int(sep.max()) + 1)
    str_ = np.isin(sep, np.where(s_tr_ep)[0])
    ste = np.isin(sep, np.where(s_te_ep)[0])
    out[prefix + "acc_in"] = _acc(sX, sy, str_, ste, N_CAUSES, ev, seed)
    sloco = {}
    for j in range(n_train_ctx):
        mte, mtr = sctx == j, sctx != j
        if mte.sum() < N_CAUSES or mtr.sum() < 20:
            continue
        sloco[str(j)] = _acc(sX, sy, mtr, mte, N_CAUSES, ev, seed)
    out[prefix + "loco"] = sloco
    out[prefix + "acc_loco"] = float(np.mean(list(sloco.values()))) \
        if sloco else float("nan")
    for key, tag in (("ood_ctx", "acc_ood_ctx"),
                     ("dense_ctx", "acc_dense_ctx")):
        sZ, sY = seg[key]["Z"], seg[key]["cause"]
        if len(sY) >= N_CAUSES:
            r = logistic_probe(sX, sy, sZ, sY, n_classes=N_CAUSES,
                               steps=ev.probe_steps, lr=ev.probe_lr,
                               seed=seed)
            out[prefix + tag] = r["acc"]
    return out


def eval_dynfeat(cfg: Config, seed: int, env_seed: int, dss: dict,
                 source: str = "canonical") -> dict:
    """Handcrafted dynamical-signature features as a pseudo-rep.

    source="canonical": headroom reference — the true pre-context signal
    (eval-only) reduced to dynamical features; shows what a perfect
    invariant concept rep could linearly expose.
    source="obs": no-learning baseline on raw observations.
    Results occupy the same probes.seg_* slots so they slot into the
    summary table next to the real representations."""
    ec, ev = cfg.env, cfg.eval
    assert source in ("canonical", "obs")
    seg = {}
    for key in ("main", "ood_ctx", "dense_ctx"):
        Xsrc = dss[key]["canonical"] if source == "canonical" \
            else dss[key]["obs"]
        nsrc = None if source == "canonical" else dss[key]["next_obs"]
        seg[key] = dynseg_dataset(Xsrc, dss[key]["actions"], dss[key],
                                  ev.warmup, next_X=nsrc)
    res = {"rep": f"dynfeat_{source}", "seed": seed,
           "env_seed": env_seed, "latent_dim": len(FEAT_NAMES),
           "probes": {}, "clustering": None, "matching": None,
           "causal": None, "ood": None, "discrete": None}
    res["probes"].update(
        _seg_probes(seg, ec.n_train_contexts, ev, seed, "seg_"))
    return res


N_ACTIONS_ = 4
EPS = 1e-12


def _base_mse(model, ds, device) -> float:
    obs = torch.as_tensor(ds["obs"], dtype=torch.float32, device=device)
    nxt = torch.as_tensor(ds["next_obs"], dtype=torch.float32,
                          device=device)
    act = torch.as_tensor(ds["actions"], dtype=torch.long, device=device)
    with torch.no_grad():
        pred, _, _ = model(obs, act)
    target = obs if model.kind == "recon" else nxt
    return float(torch.nn.functional.mse_loss(pred, target).item())


def _decode_delta(model, z: torch.Tensor, z_ref: torch.Tensor,
                  device) -> np.ndarray:
    """Map a latent difference to an obs-space difference.

    For GRU predictors the head is linear: delta = head(z) - head(z_ref)
    = W z - W z_ref. For AEs we decode centroid - reference centroid
    through the (nonlinear) decoder.
    """
    with torch.no_grad():
        if hasattr(model, "head"):
            dz = model.head(z.to(device)) - model.head(z_ref.to(device))
        elif hasattr(model, "decoder"):
            dz = model.decoder(z.to(device)) - model.decoder(
                z_ref.to(device))
        else:
            return None
    return dz.cpu().numpy()


def _intervention(model, rep, X, lab, cfg, env_seed) -> dict:
    """Per context j: latent centroid deltas d_c = cent(c,j) - cent(.,j)
    decoded through the model head should align with the cause's true
    obs-space direction C_j W v_c. Selectivity = mean(diag - max off)."""
    device = next(model.parameters()).device
    cents = {}
    for j in range(cfg.env.n_train_contexts):
        m = lab["main"]["single"] & (lab["main"]["ctx"] == j)
        cc, ids = cause_centroids(X["main"], np.where(
            lab["main"]["single"]
            & (lab["main"]["seg_pos"] >= cfg.eval.seg_warmup),
            lab["main"]["cause_a"], -1),
            lab["main"]["ctx"], j)
        if len(ids) >= 2:
            cents[j] = (cc, ids, X["main"][m].mean(0))
    proto = {j: obs_space_prototypes(cfg.env, j, env_seed)
             for j in cents}
    sel_per_ctx = {}
    for j, (cc, ids, ref) in cents.items():
        z = torch.as_tensor(cc, dtype=torch.float32)
        zr = torch.as_tensor(ref[None], dtype=torch.float32)
        d = _decode_delta(model, z, zr, device)      # (n_c, D)
        if d is None:
            return {}
        P = proto[j]                                # (D, n_causes)
        dn = d / (np.linalg.norm(d, axis=1, keepdims=True) + EPS)
        sim = dn @ P                                # (n_c, n_causes)
        n = len(ids)
        sels = []
        for i in range(n):
            true_c = ids[i]
            off = [sim[i, c] for c in range(N_CAUSES) if c != true_c]
            sels.append(float(sim[i, true_c] - max(off)))
        sel_per_ctx[str(j)] = float(np.mean(sels))
    return {"selectivity_per_ctx": sel_per_ctx,
            "selectivity": float(np.mean(list(sel_per_ctx.values())))
            if sel_per_ctx else float("nan")}


def eval_analytic(name: str, cfg: Config, seed: int, train_ds,
                  dss=None) -> dict:
    rep = build_analytic_rep(name, cfg.env.obs_dim, cfg.model, train_ds)
    return eval_representation(rep, cfg, seed, "cpu", model=None,
                               train_ds=train_ds, env_seed=seed, dss=dss)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", default=str(Path(__file__).parent /
                                            "configs/default.yaml"))
    ap.add_argument("--run-dir", default=None)
    ap.add_argument("--rep", default=None,
                    help="analytic rep name (raw|raw_win|pca|pca_win)")
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--kmeans", action="store_true",
                    help="wrap run-dir model in k-means rep")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    cfg = load_config(args.config)
    from .data import train_val_datasets
    train_ds, _ = train_val_datasets(cfg, args.seed)

    if args.rep:
        rep = build_analytic_rep(args.rep, cfg.env.obs_dim, cfg.model,
                                 train_ds)
        res = eval_representation(rep, cfg, args.seed, args.device,
                                  train_ds=train_ds, env_seed=args.seed)
        out_path = Path(args.out) if args.out else \
            REPORTS / f"eval_{args.rep}__seed{args.seed}.json"
    else:
        # prefer the best-val checkpoint (final ckpt.pt can be mildly
        # overfit late in training)
        run_dir = Path(args.run_dir)
        ckpt_path = run_dir / "ckpt_best.pt"
        if not ckpt_path.exists():
            ckpt_path = run_dir / "ckpt.pt"
        model, ck = load_ckpt(ckpt_path, args.device)
        env_seed = ck["env_seed"]
        if args.kmeans:
            rep = build_kmeans_rep(ck["model_name"], model, args.device,
                                   cfg.model, train_ds, args.seed)
        else:
            rep = TorchRep(ck["model_name"], model, args.device)
        res = eval_representation(rep, cfg, args.seed, args.device,
                                  model=model, train_ds=train_ds,
                                  env_seed=env_seed)
        out_path = Path(args.out) if args.out else \
            Path(args.run_dir) / f"eval{('_km' if args.kmeans else '')}.json"
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(json.dumps(res, indent=2, default=float))
    print(json.dumps(res, indent=2, default=float))


if __name__ == "__main__":
    main()
