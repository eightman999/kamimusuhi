"""PPO trainer for U0 need-guided memory.

Each PPO iteration rolls out `num_envs` complete episodes (all envs share
the same episode length, so a rollout is exactly T steps and every
recurrent hidden state legitimately restarts from zero). The update
re-runs the stored observation sequences through the policy, which keeps
recurrent models correct without storing hidden states.

Optional teacher bootstrapping (`imitation_iters` > 0) is
*mechanics-only*: an oracle demonstrates how the memory API and the
recall->move->act recovery sequence work, but its STORE decisions are
relabelled IGNORE before becoming BC targets, so need relevance itself
is never taught. Runs that use it are tagged "teacher" in run metadata
and must not be conflated with U0-main (pure reward) results.

Budget controls (honored without code changes):
    U0_TIME_BUDGET  seconds; training stops at the next update boundary
    U0_MAX_ITERS    hard cap on PPO iterations
    U0_NUM_ENVS     override env count

Artifacts per run: artifacts/runs/<run_id>/{config.json, metrics.jsonl,
best.pt, last.pt, done.json}. --resume continues from last.pt.

Usage:
    python -m experiments.u0.train --config experiments/u0/configs/default.yaml \
        --model gru64 --seed 0 --run-id gru64_s0
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import time
from pathlib import Path

import numpy as np
import torch
import torch.nn as nn

from .config import env_config, load_config, train_config
from .env.u0_env import IGNORE, STORE, VecU0Env
from .evaluate import evaluate_learned
from .models.nets import build_policy
from .policies.baselines import OraclePolicy


def git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"],
            cwd=Path(__file__).resolve().parents[2],
            stderr=subprocess.DEVNULL).decode().strip()
    except Exception:
        return "unknown"


def collect_rollout(policy, vec: VecU0Env, device, teacher=None,
                    teacher_mix: float = 0.5):
    """One iteration = one complete episode per env.

    When `teacher` (an OraclePolicy) is given, each step is labeled with
    the action the oracle would take in the student's state — except
    STORE, which is relabelled IGNORE so only memory *mechanics* and the
    recovery sequence are taught, never what is worth remembering.
    """
    T = vec.envs[0].cfg.episode_len
    B, D = vec.num_envs, vec.obs_dim
    obs_buf = np.zeros((B, T, D), dtype=np.float32)
    act_buf = np.zeros((B, T), dtype=np.int64)
    logp_buf = np.zeros((B, T), dtype=np.float32)
    rew_buf = np.zeros((B, T), dtype=np.float32)
    val_buf = np.zeros((B, T), dtype=np.float32)
    lab_buf = np.zeros((B, T), dtype=np.int64)
    ep_stats = []
    teachers = None
    if teacher is not None:
        # one oracle per env: each caches its own episode's need plan and
        # must reset when that env auto-resets on `done`
        teachers = [type(teacher)(seed=i) for i in range(B)]
        for te in teachers:
            te.reset()

    obs = vec.reset()
    h = policy.initial_state(B, device)
    policy.eval()
    with torch.no_grad():
        for t in range(T):
            o = torch.as_tensor(obs, device=device)
            logits, v, h = policy(o, h)
            dist = torch.distributions.Categorical(logits=logits)
            act = dist.sample()
            logp = dist.log_prob(act)
            if teachers is not None:
                for i, e in enumerate(vec.envs):
                    la = teachers[i].decide(e)
                    lab_buf[i, t] = IGNORE if la == STORE else la
                    if np.random.random() < teacher_mix:
                        act[i] = la       # execute full oracle incl. STORE
            obs_buf[:, t] = obs
            act_buf[:, t] = act.cpu().numpy()
            logp_buf[:, t] = logp.cpu().numpy()
            val_buf[:, t] = v.cpu().numpy()
            obs, rew, done, infos = vec.step(act.cpu().numpy())
            rew_buf[:, t] = rew
            for i, inf in enumerate(infos):
                if "ep_stats" in inf:
                    ep_stats.append(inf["ep_stats"])
                    if teachers is not None:
                        teachers[i].reset()
    policy.train()
    return (obs_buf, act_buf, logp_buf, rew_buf, val_buf,
            ep_stats, lab_buf)


def gae(rew: np.ndarray, val: np.ndarray, gamma: float, lam: float):
    """Episodes are complete (terminal at T-1), so no bootstrap is needed."""
    B, T = rew.shape
    adv = np.zeros_like(rew)
    last = np.zeros(B, dtype=np.float32)
    for t in reversed(range(T)):
        v_next = val[:, t + 1] if t + 1 < T else 0.0
        delta = rew[:, t] + gamma * v_next - val[:, t]
        last = delta + gamma * lam * last
        adv[:, t] = last
    return adv, adv + val


def _seq_forward(policy, obs_seq: torch.Tensor, device):
    """Re-run a (B, T, D) observation sequence; returns logits (B,T,A) and
    values (B,T). Recurrent policies start from h0=0 (episode aligned)."""
    B, T, _D = obs_seq.shape
    h = policy.initial_state(B, device)
    logits, vals = [], []
    for t in range(T):
        al, v, h = policy(obs_seq[:, t], h)
        logits.append(al)
        vals.append(v)
    return torch.stack(logits, 1), torch.stack(vals, 1)


def ppo_update(policy, opt, bufs, tc, device):
    obs_buf, act_buf, logp_buf, rew_buf, val_buf = bufs
    adv, ret = gae(rew_buf, val_buf, tc["gamma"], tc["lam"])
    B, T, D = obs_buf.shape

    obs_t = torch.as_tensor(obs_buf, device=device)
    act_t = torch.as_tensor(act_buf, device=device)
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
            al_s, v_s = _seq_forward(policy, obs_t[idx], device)
            dist = torch.distributions.Categorical(logits=al_s)
            logp = dist.log_prob(act_t[idx])
            ratio = torch.exp(logp - old_logp_t[idx])
            a = adv_t[idx]
            pi_loss = torch.max(
                -a * ratio,
                -a * torch.clamp(ratio, 1 - clip, 1 + clip)).mean()
            v_loss = 0.5 * (v_s - ret_t[idx]).pow(2).mean()
            ent_b = dist.entropy().mean()
            loss = pi_loss + vfc * v_loss - ent * ent_b
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(policy.parameters(),
                                     tc["max_grad_norm"])
            opt.step()
            pl += pi_loss.item()
            vl += v_loss.item()
            el += ent_b.item()
            nup += 1
    return {"pi_loss": pl / nup, "v_loss": vl / nup, "entropy": el / nup}


def bc_update(policy, opt, obs_buf, lab_buf, tc, device):
    """Mechanics-only behavioral cloning on oracle-labeled student steps."""
    B, T, D = obs_buf.shape
    obs_t = torch.as_tensor(obs_buf, device=device)
    lab_t = torch.as_tensor(lab_buf, device=device)
    mb = tc.get("minibatch_episodes", B)
    total = 0.0
    nup = 0
    for _ in range(tc["epochs"]):
        perm = torch.randperm(B, device=device)
        for lo in range(0, B, mb):
            idx = perm[lo:lo + mb]
            al_s, _v = _seq_forward(policy, obs_t[idx], device)
            al = al_s.reshape(-1, al_s.shape[-1])
            lab = lab_t[idx].reshape(-1)
            freq = torch.bincount(lab, minlength=al.shape[-1]).float()
            w = (1.0 / freq.clamp(min=1))
            w = w * (al.shape[-1] / w.sum())   # class-balanced act CE
            loss = torch.nn.functional.cross_entropy(al, lab, weight=w)
            opt.zero_grad()
            loss.backward()
            nn.utils.clip_grad_norm_(policy.parameters(),
                                     tc["max_grad_norm"])
            opt.step()
            total += loss.item()
            nup += 1
    return {"bc_loss": total / max(1, nup)}


def save_ckpt(policy, path: Path, **meta) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    torch.save({"model": policy.state_dict(), **meta}, path)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--model", default="gru64",
                    choices=["mlp", "gru64", "gru128"])
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--artifacts", default="experiments/u0/artifacts")
    ap.add_argument("--run-id", default=None)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--resume", action="store_true")
    args = ap.parse_args()

    torch.set_num_threads(min(8, os.cpu_count() or 1))
    cfg = load_config(args.config)
    ecfg = env_config(cfg, seed=args.seed)
    tc = train_config(cfg)
    iters = int(os.environ.get("U0_MAX_ITERS", tc["iters"]))
    num_envs = int(os.environ.get("U0_NUM_ENVS", tc["num_envs"]))
    budget = float(os.environ.get("U0_TIME_BUDGET", "0"))

    run_id = args.run_id or f"{args.model}_s{args.seed}"
    run_dir = Path(args.artifacts) / "runs" / run_id
    run_dir.mkdir(parents=True, exist_ok=True)

    torch.manual_seed(args.seed)
    np.random.seed(args.seed)
    device = torch.device(args.device)
    vec = VecU0Env(ecfg, num_envs, seed=args.seed * 977 + 13)
    policy = build_policy(args.model, vec.obs_dim).to(device)
    opt = torch.optim.Adam(policy.parameters(), lr=tc["lr"])

    start_iter = 0
    last_pt = run_dir / "last.pt"
    if args.resume and last_pt.exists():
        state = torch.load(last_pt, map_location="cpu", weights_only=True)
        policy.load_state_dict(state["model"])
        start_iter = int(state.get("iter", 0))
        metrics_path = run_dir / "metrics.jsonl"
        if metrics_path.exists():
            lines = metrics_path.read_text().strip().splitlines()
            if lines:
                start_iter = max(start_iter,
                                 int(json.loads(lines[-1])["iter"]))
        print(f"[{run_id}] resume at iter {start_iter}", flush=True)

    (run_dir / "config.json").write_text(json.dumps(
        {"env": {k: (list(v) if isinstance(v, tuple) else v)
                 for k, v in ecfg.__dict__.items()},
         "train": tc, "model": args.model, "seed": args.seed,
         "run_id": run_id, "git_commit": git_commit(),
         "torch_version": torch.__version__, "device": args.device,
         "teacher": "mechanics_oracle" if tc.get("imitation_iters") else None},
        indent=2))
    metrics_f = open(run_dir / "metrics.jsonl", "a")

    teacher = OraclePolicy(seed=args.seed) \
        if tc.get("imitation_iters", 0) else None
    best_err = float("inf")
    t0 = time.time()
    for it in range(start_iter + 1, iters + 1):
        in_imit = teacher is not None and it <= tc["imitation_iters"]
        bufs = collect_rollout(policy, vec, device,
                               teacher=teacher if in_imit else None)
        if in_imit:
            losses = bc_update(policy, opt, bufs[0], bufs[6], tc, device)
        else:
            losses = ppo_update(policy, opt, bufs[:5], tc, device)
        stats = bufs[5]
        agg = {k: float(np.nanmean([s[k] for s in stats]))
               for k in stats[0]
               if isinstance(stats[0][k], (int, float))} if stats else {}
        rec = {"iter": it, "elapsed": round(time.time() - t0, 1),
               "mean_return": float(bufs[3].sum(1).mean()),
               **{f"ep_{k}": v for k, v in agg.items()}, **losses}
        if it % tc["eval_every"] == 0 or it == iters:
            val = evaluate_learned(policy, ecfg, args.device,
                                   episodes=tc["eval_episodes"],
                                   seed=tc["val_seed"])
            rec["val_error_full"] = val["error_full"]
            rec["val_need_resolution"] = val["need_resolution"]
            rec["val_important_retention"] = val["important_retention"]
            rec["val_store_precision"] = val["store_precision"]
            if val["error_full"] < best_err:
                best_err = val["error_full"]
                save_ckpt(policy, run_dir / "best.pt",
                          model_name=args.model, iter=it,
                          val_error_full=best_err)
        if it % tc.get("checkpoint_every", 10) == 0 or it == iters:
            save_ckpt(policy, last_pt, model_name=args.model, iter=it)
        metrics_f.write(json.dumps(rec) + "\n")
        metrics_f.flush()
        print(f"[{run_id}] it {it}/{iters} ret {rec['mean_return']:.3f} "
              f"err {agg.get('error_full', float('nan')):.4f} "
              f"ret_res {agg.get('need_resolution', 0):.2f} "
              f"val {rec.get('val_error_full', float('nan')):.4f}",
              flush=True)
        if budget and time.time() - t0 > budget:
            break

    save_ckpt(policy, last_pt, model_name=args.model, iter=it)
    (run_dir / "done.json").write_text(json.dumps(
        {"iters": it, "best_val_error_full": best_err,
         "elapsed_sec": round(time.time() - t0, 1),
         "device": args.device,
         "torch_num_threads": torch.get_num_threads(),
         "git_commit": git_commit()}, indent=2))
    print(f"[{run_id}] done iters={it} best_val_err={best_err:.4f}")


if __name__ == "__main__":
    main()
