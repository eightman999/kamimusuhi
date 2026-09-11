"""PPO trainer for the R0 learned memory gate.

Each PPO iteration rolls out `num_envs` complete episodes (all envs share
the same episode length, so a rollout is exactly T steps and every hidden
state legitimately restarts from zero). The update re-runs the stored
observation sequences through the policy, which keeps recurrent models
correct without storing hidden states.

Budget controls (for smoke runs, honored without code changes):
    R0_TIME_BUDGET  seconds; training stops at the next update boundary
    R0_MAX_ITERS    hard cap on PPO iterations
    R0_NUM_ENVS     override env count

Usage:
    python -m experiments.r0.train --config experiments/r0/configs/default.yaml \
        --model gru64 --seed 0 --artifacts experiments/r0/artifacts --run-id g64_s0
"""

from __future__ import annotations

import argparse
import json
import os
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn
import yaml

from .agents.baselines import OracleGate
from .agents.gates import build_policy, factorized_logp, factorized_entropy
from .env.r0_env import R0Config, VecR0Env, ANSWER
from .evaluate import evaluate_learned

ANS_IGNORE = -100


def load_config(path: str) -> dict:
    with open(path) as f:
        return yaml.safe_load(f)


def collect_rollout(policy, vec: VecR0Env, device, teacher=None,
                    teacher_mix: float = 0.5):
    """One iteration = one complete episode per env.

    When `teacher` (an OracleGate) is given, each step is labeled with the
    action the oracle would take *in the student's current state* plus the
    true answer when it is recoverable from the student's memory — a
    DAGGER-style supervision signal for the BC warmup.
    """
    T = vec.envs[0].cfg.episode_len
    B, D = vec.num_envs, vec.obs_dim
    obs_buf = np.zeros((B, T, D), dtype=np.float32)
    act_buf = np.zeros((B, T), dtype=np.int64)
    ans_buf = np.zeros((B, T), dtype=np.int64)
    logp_buf = np.zeros((B, T), dtype=np.float32)
    rew_buf = np.zeros((B, T), dtype=np.float32)
    val_buf = np.zeros((B, T), dtype=np.float32)
    alab_buf = np.zeros((B, T), dtype=np.int64)
    anlab_buf = np.full((B, T), ANS_IGNORE, dtype=np.int64)
    ep_stats = []
    if teacher is not None:
        teacher.reset()

    obs = vec.reset()
    h = policy.initial_state(B, device)
    policy.eval()
    with torch.no_grad():
        for t in range(T):
            o = torch.as_tensor(obs, device=device)
            al, nl, v, h = policy(o, h)
            ad = torch.distributions.Categorical(logits=al)
            nd = torch.distributions.Categorical(logits=nl)
            act = ad.sample()
            ans = nd.sample()
            if teacher is not None:
                for i, e in enumerate(vec.envs):
                    la, _lans = teacher.decide(e)
                    alab_buf[i, t] = la
                    p = e.pending
                    if (p is not None and p["recalled"]
                            and e.memory.find_key(p["key"]) is not None):
                        anlab_buf[i, t] = p["value"]
                    # execute the teacher's action on a fraction of envs so
                    # rollouts visit healthy memory states (DAGGER coverage)
                    if np.random.random() < teacher_mix:
                        act[i] = la
                        if la == ANSWER:
                            ans[i] = e.answer_key_match(teacher.rng)
            # Force-answer at deadline: the env demands an answer when the
            # query window closes, so every query yields a correct/wrong
            # gradient instead of letting the agent expire all queries.
            for i, e in enumerate(vec.envs):
                if e.pending is not None and e.t == e.pending["deadline"]:
                    act[i] = ANSWER
            logp, is_ans = factorized_logp(al, nl, act, ans)
            obs_buf[:, t] = obs
            act_buf[:, t] = act.cpu().numpy()
            ans_buf[:, t] = ans.cpu().numpy()
            logp_buf[:, t] = logp.cpu().numpy()
            val_buf[:, t] = v.cpu().numpy()
            obs, rew, done, infos = vec.step(act.cpu().numpy(), ans.cpu().numpy())
            rew_buf[:, t] = rew
            for i in infos:
                if "ep_stats" in i:
                    ep_stats.append(i["ep_stats"])
    policy.train()
    return (obs_buf, act_buf, ans_buf, logp_buf, rew_buf, val_buf,
            ep_stats, alab_buf, anlab_buf)


def gae(rew: np.ndarray, val: np.ndarray, gamma: float, lam: float):
    B, T = rew.shape
    adv = np.zeros_like(rew)
    last = np.zeros(B, dtype=np.float32)
    for t in reversed(range(T)):
        v_next = val[:, t + 1] if t + 1 < T else 0.0
        delta = rew[:, t] + gamma * v_next - val[:, t]
        last = delta + gamma * lam * last
        adv[:, t] = last
    return adv, adv + val


def ppo_update(policy, opt, bufs, tc, device):
    obs_buf, act_buf, ans_buf, logp_buf, rew_buf, val_buf = bufs
    adv, ret = gae(rew_buf, val_buf, tc["gamma"], tc["lam"])
    B, T, D = obs_buf.shape

    obs_t = torch.as_tensor(obs_buf, device=device)
    act_t = torch.as_tensor(act_buf, device=device)
    ans_t = torch.as_tensor(ans_buf, device=device)
    old_logp_t = torch.as_tensor(logp_buf, device=device)
    adv_t = torch.as_tensor(adv, device=device)
    ret_t = torch.as_tensor(ret, device=device)
    adv_t = (adv_t - adv_t.mean()) / (adv_t.std() + 1e-8)

    clip, vfc, ent = tc["clip"], tc["vf_coef"], tc["ent_coef"]
    mb = tc.get("minibatch_episodes", B)
    pl = vl = el = 0.0
    nup = 0
    for _ in range(tc["epochs"]):
        perm = torch.randperm(B, device=device)
        for lo in range(0, B, mb):
            idx = perm[lo:lo + mb]
            n = len(idx)
            h = policy.initial_state(n, device)
            al_seq, nl_seq, v_seq = [], [], []
            for t in range(T):
                al, nl, v, h = policy(obs_t[idx, t], h)
                al_seq.append(al)
                nl_seq.append(nl)
                v_seq.append(v)
            al_s = torch.stack(al_seq, 1)
            nl_s = torch.stack(nl_seq, 1)
            v_s = torch.stack(v_seq, 1)
            logp, _ = factorized_logp(al_s, nl_s, act_t[idx], ans_t[idx])
            ratio = torch.exp(logp - old_logp_t[idx])
            a = adv_t[idx]
            pi_loss = torch.max(-a * ratio,
                                -a * torch.clamp(ratio, 1 - clip, 1 + clip)).mean()
            v_loss = 0.5 * (v_s - ret_t[idx]).pow(2).mean()
            ent_b = factorized_entropy(al_s, nl_s)
            loss = pi_loss + vfc * v_loss - ent * ent_b
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(policy.parameters(), tc["max_grad_norm"])
            opt.step()
            pl += pi_loss.item()
            vl += v_loss.item()
            el += ent_b.item()
            nup += 1
    return {"pi_loss": pl / nup, "v_loss": vl / nup, "entropy": el / nup}


def bc_update(policy, opt, obs_buf, alab_buf, anlab_buf, tc, device):
    """Behavioral cloning on oracle-labeled student trajectories."""
    B, T, D = obs_buf.shape
    obs_t = torch.as_tensor(obs_buf, device=device)
    alab_t = torch.as_tensor(alab_buf, device=device)
    anlab_t = torch.as_tensor(anlab_buf, device=device)
    mb = tc.get("minibatch_episodes", B)
    total = 0.0
    nup = 0
    for _ in range(tc["epochs"]):
        perm = torch.randperm(B, device=device)
        for lo in range(0, B, mb):
            idx = perm[lo:lo + mb]
            n = len(idx)
            h = policy.initial_state(n, device)
            al_seq, nl_seq = [], []
            for t in range(T):
                al, nl, _v, h = policy(obs_t[idx, t], h)
                al_seq.append(al)
                nl_seq.append(nl)
            al_s = torch.stack(al_seq, 1).reshape(-1, al_seq[0].shape[-1])
            nl_s = torch.stack(nl_seq, 1).reshape(-1, nl_seq[0].shape[-1])
            lab = alab_t[idx].reshape(-1)
            freq = torch.bincount(lab, minlength=al_s.shape[-1]).float()
            w = (1.0 / freq.clamp(min=1))
            w = w * (al_s.shape[-1] / w.sum())   # class-balanced act CE
            loss_a = torch.nn.functional.cross_entropy(al_s, lab, weight=w)
            loss_n = torch.nn.functional.cross_entropy(
                nl_s, anlab_t[idx].reshape(-1), ignore_index=ANS_IGNORE)
            loss = loss_a + loss_n
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(policy.parameters(), tc["max_grad_norm"])
            opt.step()
            total += loss.item()
            nup += 1
    return {"bc_loss": total / max(1, nup)}


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--model", default="gru64", choices=["mlp", "gru64", "gru128"])
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--artifacts", default="experiments/r0/artifacts")
    ap.add_argument("--run-id", default=None)
    ap.add_argument("--device", default="cpu")
    args = ap.parse_args()

    cfg = load_config(args.config)
    ecfg = R0Config(**{**cfg.get("env", {}), "seed": args.seed})
    tc = cfg["train"]
    iters = int(os.environ.get("R0_MAX_ITERS", tc["iters"]))
    num_envs = int(os.environ.get("R0_NUM_ENVS", tc["num_envs"]))
    budget = float(os.environ.get("R0_TIME_BUDGET", "0"))

    run_id = args.run_id or f"{args.model}_s{args.seed}_{int(time.time())}"
    run_dir = Path(args.artifacts) / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)

    torch.manual_seed(args.seed)
    np.random.seed(args.seed)
    device = torch.device(args.device)
    vec = VecR0Env(ecfg, num_envs, seed=args.seed * 977 + 13)
    policy = build_policy(args.model, vec.obs_dim, ecfg.num_values).to(device)
    opt = torch.optim.Adam(policy.parameters(), lr=tc["lr"])

    (run_dir / "config.json").write_text(json.dumps(
        {"env": ecfg.__dict__, "train": tc, "model": args.model,
         "seed": args.seed, "run_id": run_id}, indent=2))
    metrics_f = open(run_dir / "metrics.jsonl", "a")

    teacher = OracleGate(seed=args.seed) if tc.get("imitation_iters", 0) else None
    best_acc = -1.0
    t0 = time.time()
    warm = tc.get("delay_warmup_iters", 0)
    d_lo, d_hi = tc.get("delay_start", [4, 12])
    for it in range(1, iters + 1):
        if warm:
            frac = min(1.0, it / warm)
            for e in vec.envs:
                e.cfg.delay_min = round(d_lo + frac * (ecfg.delay_min - d_lo))
                e.cfg.delay_max = round(d_hi + frac * (ecfg.delay_max - d_hi))
        in_imit = teacher is not None and it <= tc["imitation_iters"]
        bufs = collect_rollout(policy, vec, device,
                               teacher=teacher if in_imit else None)
        if in_imit:
            losses = bc_update(policy, opt, bufs[0], bufs[7], bufs[8],
                               tc, device)
        else:
            losses = ppo_update(policy, opt, bufs[:6], tc, device)
        stats = bufs[6]
        agg = {k: float(np.mean([s[k] for s in stats])) for k in stats[0]} \
            if stats else {}
        rec = {"iter": it, "elapsed": round(time.time() - t0, 1),
               "mean_return": float(bufs[4].sum(1).mean()),
               **{f"ep_{k}": v for k, v in agg.items()}, **losses}
        if it % tc["eval_every"] == 0 or it == iters:
            val = evaluate_learned(policy, ecfg, device,
                                   episodes=tc["eval_episodes"],
                                   seed=tc["val_seed"])
            rec["val_accuracy"] = val["accuracy"]
            if val["accuracy"] > best_acc:
                best_acc = val["accuracy"]
                torch.save({"model": policy.state_dict(),
                            "model_name": args.model, "env": ecfg.__dict__},
                           run_dir / "best.pt")
        metrics_f.write(json.dumps(rec) + "\n")
        metrics_f.flush()
        print(f"[{run_id}] it {it} ret {rec['mean_return']:.3f} "
              f"acc {agg.get('accuracy', 0):.3f} ret? "
              f"{agg.get('important_retention', 0):.3f} "
              f"val {rec.get('val_accuracy', float('nan')):.3f}", flush=True)
        if budget and time.time() - t0 > budget:
            break

    torch.save({"model": policy.state_dict(),
                "model_name": args.model, "env": ecfg.__dict__},
               run_dir / "last.pt")
    (run_dir / "done.json").write_text(json.dumps(
        {"iters": it, "best_val_accuracy": best_acc,
         "elapsed_sec": time.time() - t0}, indent=2))
    print(f"[{run_id}] done iters={it} best_val={best_acc:.3f}")


if __name__ == "__main__":
    main()
