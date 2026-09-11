"""U0 training: supervised classifier + confidence net, then PPO on the
meta-policy that turns the uncertainty vector into ANSWER/OBSERVE/ABSTAIN.

Phase 1 (supervised): episodes are rolled out with a coverage policy that
always observes to the budget, recording (obs_n, y) at every decision
point.  The classifier trains with cross-entropy; the confidence net then
trains to predict P(classifier argmax == y), frozen classifier.  The
ensemble and MC-dropout baselines train on the same data.

Phase 2 (PPO): MetaPolicy sees ONLY the uncertainty feature vector
u = [conf, margin, var_z, n_frac] computed by the frozen nets, so the
causal tests can surgically perturb exactly what the decision head
consumes.  The class choice under ANSWER is the frozen classifier argmax.

Budget controls (honored without code changes for smoke runs):
    U0_TIME_BUDGET  seconds, stops at the next update boundary
    U0_MAX_ITERS    hard cap on PPO iterations
    U0_NUM_ENVS     rollout env count
    U0_SUP_EPISODES supervised dataset size

Usage:
    python -m experiments.u0.train --config experiments/u0/configs/default.yaml \
        --seed 0 --artifacts experiments/u0/artifacts --run-id s0
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import yaml

from .env.uncertainty_env import (ABSTAIN, ANSWER, OBSERVE, U0Config,
                                  VecU0Env)
from .models.nets import (Classifier, ConfNet, DropoutClassifier,
                          META_ABSTAIN, META_ANSWER, META_OBSERVE,
                          MetaPolicy, U_DIM, uncertainty_features)


def atomic(path, obj):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(obj, indent=2, allow_nan=False) + "\n")
    tmp.replace(path)


def file_sha256(path) -> str:
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def load_config(path):
    with open(path) as f:
        return yaml.safe_load(f)


# ---------------------------------------------------------------------------
# Phase 1: supervised data + classifier / confidence / ensemble
# ---------------------------------------------------------------------------

def collect_supervised(cfg: U0Config, episodes: int, seed: int):
    """Roll a full-observe policy; record (obs at every n, y, regime)."""
    from .env.uncertainty_env import U0Env
    env = U0Env(U0Config(**{**cfg.__dict__, "seed": seed}))
    rows, labels, regimes, ns = [], [], [], []
    for ep in range(episodes):
        obs = env.reset()
        rows.append(obs.copy())
        labels.append(env.latent["y"])
        regimes.append(env.latent["regime"])
        ns.append(env.n)
        while not env.done:
            obs, _, done, _ = env.step(cfg.observe_id)
            if not done:
                rows.append(obs.copy())
                labels.append(env.latent["y"])
                regimes.append(env.latent["regime"])
                ns.append(env.n)
    return {"obs": np.asarray(rows), "y": np.asarray(labels),
            "regime": np.asarray(regimes), "n": np.asarray(ns)}


def train_supervised(obs, y, model, epochs, batch, lr, device, seed,
                     target_fn=None, log_every=1):
    """Generic CE/BCE loop.  target_fn(obs,y,model)->target overrides y."""
    model.to(device)
    opt = torch.optim.Adam(model.parameters(), lr=lr)
    gen = torch.Generator().manual_seed(seed)
    obs_t = torch.as_tensor(obs, dtype=torch.float32, device=device)
    y_t = torch.as_tensor(y, dtype=torch.long, device=device)
    n = len(obs_t)
    hist = []
    is_conf = target_fn is not None
    for ep in range(epochs):
        perm = torch.randperm(n, generator=gen).to(device)
        tot = 0.0
        for lo in range(0, n, batch):
            idx = perm[lo:lo + batch]
            out = model(obs_t[idx])
            if is_conf:
                tgt = target_fn(obs_t[idx], y_t[idx], model)
                loss = nn.functional.binary_cross_entropy(
                    out.clamp(1e-6, 1 - 1e-6), tgt)
            else:
                loss = nn.functional.cross_entropy(out, y_t[idx])
            opt.zero_grad()
            loss.backward()
            opt.step()
            tot += loss.item() * len(idx)
        hist.append(tot / n)
        if ep % log_every == 0:
            print(f"    epoch {ep} loss {tot / n:.4f}", flush=True)
    return hist


def make_conf_target(clf):
    def fn(obs, y, _model):
        with torch.no_grad():
            return (clf(obs).argmax(-1) == y).float()
    return fn


# ---------------------------------------------------------------------------
# Phase 1b: imitation from the oracle (stabilizes PPO; t0 convention)
# ---------------------------------------------------------------------------

def collect_imitation(ecfg: U0Config, clf, conf_net, episodes: int,
                      seed: int, device):
    """(u, oracle_meta_action) pairs at EVERY decision point of episodes
    rolled out with full observation, so all n are covered."""
    from .env.uncertainty_env import U0Env
    from .policies.oracle import BayesOracle
    env = U0Env(U0Config(**{**ecfg.__dict__, "seed": seed}))
    oracle = BayesOracle(ecfg)
    us, labels = [], []
    for _ in range(episodes):
        obs = env.reset()
        n_star = oracle.plan(env)
        while True:
            o = torch.as_tensor(obs, dtype=torch.float32,
                                device=device)[None]
            with torch.no_grad():
                u = uncertainty_features(clf(o), conf_net(o), o,
                                         ecfg.feat_dim)[0]
            us.append(u.cpu())
            if n_star == 0 or env.n >= ecfg.max_obs:
                lab = META_ABSTAIN if n_star == 0 else META_ANSWER
            elif env.n < n_star:
                lab = META_OBSERVE
            else:
                lab = META_ANSWER
            labels.append(lab)
            if env.done:
                break
            obs, _, done, _ = env.step(ecfg.observe_id)
            if done:
                break
    return torch.stack(us), torch.as_tensor(labels, dtype=torch.long)


def imitation_update(meta, opt, us, labels, epochs, batch, device, seed):
    gen = torch.Generator().manual_seed(seed)
    us = us.to(device)
    labels = labels.to(device)
    freq = torch.bincount(labels, minlength=3).float().clamp_min(1)
    weights = (freq.sum() / freq).sqrt()
    weights /= weights.mean()
    n = len(us)
    last = 0.0
    for _ in range(epochs):
        perm = torch.randperm(n, generator=gen).to(device)
        for lo in range(0, n, batch):
            idx = perm[lo:lo + batch]
            logits, _ = meta(us[idx])
            loss = nn.functional.cross_entropy(logits, labels[idx],
                                               weight=weights)
            opt.zero_grad()
            loss.backward()
            opt.step()
            last = loss.item()
    return last


# ---------------------------------------------------------------------------
# Phase 2: PPO on the meta-policy over u
# ---------------------------------------------------------------------------

def meta_to_env_action(meta_act: torch.Tensor, class_choice: torch.Tensor,
                       cfg: U0Config) -> torch.Tensor:
    return torch.where(
        meta_act == META_ANSWER, class_choice,
        torch.where(meta_act == META_OBSERVE,
                    torch.full_like(class_choice, cfg.observe_id),
                    torch.full_like(class_choice, cfg.abstain_id)))


def collect_rollout(meta, clf, conf_net, vec, cfg, device):
    """One iteration = one episode per env (<= max_obs steps, masked)."""
    T, B = cfg.max_obs, vec.num_envs
    u_buf = torch.zeros(B, T, U_DIM)
    act_buf = torch.zeros(B, T, dtype=torch.long)
    logp_buf = torch.zeros(B, T)
    rew_buf = torch.zeros(B, T)
    val_buf = torch.zeros(B, T)
    mask = torch.zeros(B, T)
    obs = torch.as_tensor(vec.reset(), dtype=torch.float32, device=device)
    alive = torch.ones(B, dtype=torch.bool)
    meta.eval()
    with torch.no_grad():
        for t in range(T):
            if not bool(alive.any()):
                break
            logits_c = clf(obs)
            conf = conf_net(obs)
            u = uncertainty_features(logits_c, conf, obs, cfg.feat_dim)
            ml, v = meta(u)
            dist = torch.distributions.Categorical(logits=ml)
            ma = dist.sample()
            env_act = meta_to_env_action(ma, logits_c.argmax(-1), cfg)
            nxt, rew, done, _ = vec.step(env_act.cpu().numpy())
            u_buf[:, t] = u.cpu()
            act_buf[:, t] = ma.cpu()
            logp_buf[:, t] = dist.log_prob(ma).cpu()
            val_buf[:, t] = v.cpu()
            rew_buf[:, t] = torch.as_tensor(rew)
            mask[:, t] = alive.float()
            alive = alive & ~torch.as_tensor(done)
            obs = torch.as_tensor(nxt, dtype=torch.float32, device=device)
    meta.train()
    return u_buf, act_buf, logp_buf, rew_buf, val_buf, mask


def gae(rew, val, mask, gamma, lam):
    B, T = rew.shape
    adv = torch.zeros_like(rew)
    last = torch.zeros(B)
    for t in reversed(range(T)):
        v_next = val[:, t + 1] if t + 1 < T else torch.zeros(B)
        delta = (rew[:, t] + gamma * v_next - val[:, t]) * mask[:, t]
        last = delta + gamma * lam * last * mask[:, t]
        adv[:, t] = last
    return adv, adv + val


def ppo_update(meta, opt, bufs, tc, device):
    u_buf, act_buf, logp_buf, rew_buf, val_buf, mask = bufs
    adv, ret = gae(rew_buf, val_buf, mask, tc["gamma"], tc["lam"])
    u_t = u_buf.to(device)
    act_t = act_buf.to(device)
    old_logp = logp_buf.to(device)
    m = mask.to(device)
    adv = (adv - adv[m.bool()].mean()) / (adv[m.bool()].std() + 1e-8)
    clip = tc["clip"]
    pl = vl = el = 0.0
    nup = 0
    B = u_t.shape[0]
    for _ in range(tc["ppo_epochs"]):
        perm = torch.randperm(B, device=device)
        for lo in range(0, B, tc["minibatch_envs"]):
            idx = perm[lo:lo + tc["minibatch_envs"]]
            logits, v = meta(u_t[idx].reshape(-1, U_DIM))
            T = u_t.shape[1]
            logits = logits.reshape(len(idx), T, -1)
            v = v.reshape(len(idx), T)
            dist = torch.distributions.Categorical(logits=logits)
            mm = m[idx]
            ratio = (dist.log_prob(act_t[idx]) - old_logp[idx]).exp()
            a = adv[idx].to(device)
            pol = -torch.minimum(ratio * a,
                                 ratio.clamp(1 - clip, 1 + clip) * a)
            pol = (pol * mm).sum() / mm.sum().clamp_min(1)
            vloss = ((v - ret[idx].to(device)).square() * mm).sum() \
                / mm.sum().clamp_min(1)
            ent = (dist.entropy() * mm).sum() / mm.sum().clamp_min(1)
            loss = pol + tc["vf_coef"] * vloss - tc["ent_coef"] * ent
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(meta.parameters(),
                                     tc["max_grad_norm"])
            opt.step()
            pl += pol.item()
            vl += vloss.item()
            el += ent.item()
            nup += 1
    return {"pi_loss": pl / nup, "v_loss": vl / nup, "entropy": el / nup}


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--artifacts", default="experiments/u0/artifacts")
    ap.add_argument("--run-id", default=None)
    ap.add_argument("--device", default="cpu")
    args = ap.parse_args()

    cfg = load_config(args.config)
    ecfg = U0Config(**{**cfg.get("env", {}), "seed": args.seed})
    tc = cfg["train"]
    dc = cfg["data"]
    iters = int(os.environ.get("U0_MAX_ITERS", tc["ppo_iters"]))
    num_envs = int(os.environ.get("U0_NUM_ENVS", tc["num_envs"]))
    sup_eps = int(os.environ.get("U0_SUP_EPISODES", dc["episodes"]))
    budget = float(os.environ.get("U0_TIME_BUDGET", "0"))

    run_id = args.run_id or f"s{args.seed}_{int(time.time())}"
    run_dir = Path(args.artifacts) / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)

    torch.manual_seed(args.seed)
    np.random.seed(args.seed)
    device = torch.device(args.device)
    t0 = time.time()

    commit = "unknown"
    try:
        commit = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], text=True).strip()
    except Exception:
        pass

    # ----- phase 1: supervised models ---------------------------------
    print(f"[{run_id}] collecting supervised data ({sup_eps} episodes)")
    ds = collect_supervised(ecfg, sup_eps, dc["seed"] + args.seed)
    val_ds = collect_supervised(ecfg, dc["val_episodes"],
                                dc["seed"] + 777 + args.seed)
    obs_dim, K = ecfg.obs_dim, ecfg.n_classes
    hid = tc["hidden"]

    clf = Classifier(obs_dim, K, hid)
    print(f"[{run_id}] training classifier")
    train_supervised(ds["obs"], ds["y"], clf, tc["cls_epochs"],
                     tc["batch"], tc["lr"], device, args.seed)

    conf_net = ConfNet(obs_dim, hid)
    print(f"[{run_id}] training confidence net")
    train_supervised(ds["obs"], ds["y"], conf_net, tc["conf_epochs"],
                     tc["batch"], tc["lr"], device, args.seed,
                     target_fn=make_conf_target(clf))

    members = []
    rng = np.random.default_rng(args.seed + 31)
    for m_i in range(tc.get("ensemble_size", 3)):
        boot = rng.integers(0, len(ds["obs"]), len(ds["obs"]))
        m = Classifier(obs_dim, K, hid)
        train_supervised(ds["obs"][boot], ds["y"][boot], m,
                         tc["cls_epochs"], tc["batch"], tc["lr"], device,
                         args.seed + 100 + m_i)
        members.append(m)

    dclf = DropoutClassifier(obs_dim, K, hid)
    print(f"[{run_id}] training dropout classifier")
    train_supervised(ds["obs"], ds["y"], dclf, tc["cls_epochs"],
                     tc["batch"], tc["lr"], device, args.seed + 55)

    # validation accuracy of the classifier
    clf.eval()
    with torch.no_grad():
        vo = torch.as_tensor(val_ds["obs"], dtype=torch.float32)
        va = clf(vo).argmax(-1).numpy()
    val_acc = float((va == val_ds["y"]).mean())

    # ----- phase 2: imitation warmup + PPO meta-policy ------------------
    from .evaluate import run_eval
    from .policies.baselines import LearnedPolicy
    meta = MetaPolicy(U_DIM, tc["u_hidden"]).to(device)
    if tc.get("imit_episodes", 0):
        us, labels = collect_imitation(ecfg, clf, conf_net,
                                       tc["imit_episodes"],
                                       dc["seed"] + 4242 + args.seed,
                                       device)
        opt_i = torch.optim.Adam(meta.parameters(), lr=tc["imit_lr"])
        l = imitation_update(meta, opt_i, us, labels, tc["imit_epochs"],
                             tc["batch"], device, args.seed)
        print(f"[{run_id}] imitation done loss={l:.4f}", flush=True)
    opt = torch.optim.Adam(meta.parameters(), lr=tc["ppo_lr"])
    vec = VecU0Env(ecfg, num_envs, seed=args.seed * 977 + 13)

    manifest = {"env": {k: (list(v) if isinstance(v, tuple) else v)
                        for k, v in ecfg.__dict__.items()},
                "train": tc, "data": dc, "seed": args.seed,
                "run_id": run_id, "device": args.device,
                "source_commit": commit, "val_classifier_acc": val_acc}
    atomic(run_dir / "config.json", manifest)
    metrics_f = open(run_dir / "metrics.jsonl", "a")

    def save_policy(path, stage, update):
        torch.save({"meta": meta.state_dict(),
                    "clf": clf.state_dict(), "conf": conf_net.state_dict(),
                    "u_dim": U_DIM, "hidden": tc["u_hidden"],
                    "obs_dim": obs_dim, "n_classes": K,
                    "clf_hidden": hid, "stage": stage, "update": update,
                    "env": manifest["env"], "seed": args.seed,
                    "source_commit": commit}, path)
        return file_sha256(path)

    torch.save({"clf": clf.state_dict(), "conf": conf_net.state_dict(),
                "ensemble": [m.state_dict() for m in members],
                "dropout_clf": dclf.state_dict(),
                "obs_dim": obs_dim, "n_classes": K, "hidden": hid,
                "env": manifest["env"], "seed": args.seed},
               run_dir / "supervised.pt")

    best = -float("inf")
    best_info = {}
    pol = LearnedPolicy(clf, conf_net, meta, ecfg)
    for it in range(1, iters + 1):
        bufs = collect_rollout(meta, clf, conf_net, vec, ecfg, device)
        losses = ppo_update(meta, opt, bufs, tc, device)
        rec = {"iter": it, "elapsed": round(time.time() - t0, 1),
               "mean_return": float(bufs[3].sum(1).mean().item()),
               **losses}
        if it % tc["eval_every"] == 0 or it == iters:
            res = run_eval(pol, ecfg, episodes=tc["eval_episodes"],
                           seed=tc["val_seed"], device=args.device,
                           batch=64, keep_records=False)
            mr = res["metrics"]["mean_reward"]
            rec["val_reward"] = mr
            rec["val_answer_rate"] = res["metrics"]["answer_rate"]
            if mr > best:
                best = mr
                sha = save_policy(run_dir / "policy_best.pt", "ppo", it)
                best_info = {"iter": it, "val_reward": mr, "sha256": sha}
                atomic(run_dir / "selection.json",
                       {"criterion": "val mean_reward",
                        "val_seed": tc["val_seed"], **best_info})
        metrics_f.write(json.dumps(rec) + "\n")
        metrics_f.flush()
        if it % 10 == 0:
            print(f"[{run_id}] it {it} ret {rec['mean_return']:.3f} "
                  f"val {rec.get('val_reward', float('nan')):.3f}",
                  flush=True)
        if budget and time.time() - t0 > budget:
            break

    final_sha = save_policy(run_dir / "policy_final.pt", "ppo", it)
    atomic(run_dir / "done.json",
           {"iters": it, "best": best_info, "final_sha256": final_sha,
            "elapsed_sec": time.time() - t0})
    print(f"[{run_id}] done iters={it} best_val={best:.3f}")


if __name__ == "__main__":
    main()
