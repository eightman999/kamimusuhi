"""PPO training for H0.

Saves three checkpoints per run (spec section 11 — never hide regression):

    pre.pt    random init, before any PPO update
    best.pt   lowest periodic-eval homeostatic error
    final.pt  after the last update

Plus train_log.json (per-update stats + eval history) and meta.json
(reproducibility: seed, git commit, config, device, torch version,
wall time, training steps, checkpoint hashes).

Usage:
    python -m experiments.h0.train --config configs/base.yaml \
        --agent gru --hidden 64 --seed 0 --out runs/gru64/seed0
"""

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import time
from pathlib import Path
from typing import Dict, List, Optional

import numpy as np
import torch
import torch.nn as nn

from .agents.models import build_model
from .config import Config, load_config, save_config
from .env import dynamics as dyn
from .env.homeostasis_env import HomeostasisEnv
from .evaluate import TorchPolicy, evaluate

CHUNK_LEN = 64  # BPTT chunk length for recurrent PPO


def git_commit() -> str:
    try:
        return (
            subprocess.check_output(
                ["git", "rev-parse", "HEAD"],
                cwd=Path(__file__).resolve().parents[2],
                stderr=subprocess.DEVNULL,
            )
            .decode()
            .strip()
        )
    except Exception:
        return "unknown"


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    h.update(path.read_bytes())
    return h.hexdigest()


def save_checkpoint(model, path: Path, **meta) -> str:
    path.parent.mkdir(parents=True, exist_ok=True)
    torch.save({"model": model.state_dict(), **meta}, path)
    return sha256_file(path)


# ---------------------------------------------------------------------------
# Vectorized rollout
# ---------------------------------------------------------------------------


class VecEnvs:
    """Synchronous vector of HomeostasisEnv; envs persist across rollouts."""

    def __init__(self, cfg: Config, n: int, seed: int):
        self.envs = [
            HomeostasisEnv(cfg.env, seed=seed * 10_000 + i) for i in range(n)
        ]
        self.n = n
        self._ep_counter = 0
        for e in self.envs:
            e.reset()

    def obs(self) -> np.ndarray:
        return np.stack([e._obs() for e in self.envs])

    def step(self, actions: np.ndarray):
        """Step all envs; auto-reset finished ones and return the fresh obs."""
        obs, rew, done = [], [], []
        for i, e in enumerate(self.envs):
            o, r, d, _ = e.step(int(actions[i]))
            if d:
                self._ep_counter += 1
                o = e.reseed(e.seed_value + 1)
            obs.append(o)
            rew.append(r)
            done.append(d)
        return np.stack(obs), np.array(rew), np.array(done)


# ---------------------------------------------------------------------------
# PPO
# ---------------------------------------------------------------------------


def collect_rollout(model, vec: VecEnvs, cfg: Config, device: str):
    """Collect rollout_steps//num_envs steps per env. Returns buffers."""
    T = cfg.train.rollout_steps // cfg.train.num_envs
    N = cfg.train.num_envs
    recurrent = model.recurrent
    H = getattr(model, "hidden_size", 0)
    n_chunks = T // CHUNK_LEN if recurrent else 0

    obs_buf = np.zeros((T, N, dyn.OBS_DIM), dtype=np.float32)
    act_buf = np.zeros((T, N), dtype=np.int64)
    logp_buf = np.zeros((T, N), dtype=np.float32)
    val_buf = np.zeros((T, N), dtype=np.float32)
    rew_buf = np.zeros((T, N), dtype=np.float32)
    done_buf = np.zeros((T, N), dtype=np.float32)  # episode ended AT step t
    h_starts = (
        torch.zeros(n_chunks, 1, N, H, device=device) if recurrent else None
    )

    obs = torch.as_tensor(vec.obs(), dtype=torch.float32, device=device)
    hidden = getattr(collect_rollout, "_hidden", None)
    if recurrent and (hidden is None or hidden.shape[2] != H):
        hidden = torch.zeros(1, N, H, device=device)

    with torch.no_grad():
        for t in range(T):
            if recurrent and t % CHUNK_LEN == 0:
                h_starts[t // CHUNK_LEN] = hidden
            # forward returns (logits, value, new_hidden); for the GRU the
            # value is computed from h_{t+1}=f(obs_t, h_t), i.e. V(s_t).
            logits, value, new_hidden = model(obs, hidden)
            dist = torch.distributions.Categorical(logits=logits)
            action = dist.sample()
            logp = dist.log_prob(action)

            obs_buf[t] = obs.cpu().numpy()
            act_buf[t] = action.cpu().numpy()
            logp_buf[t] = logp.cpu().numpy()
            val_buf[t] = value.cpu().numpy()

            next_obs, rew, done = vec.step(action.cpu().numpy())
            rew_buf[t] = rew
            done_buf[t] = done

            hidden = new_hidden
            if recurrent and done.any():
                mask = torch.as_tensor(1.0 - done, dtype=torch.float32, device=device)
                hidden = hidden * mask.view(1, N, 1)
            obs = torch.as_tensor(next_obs, dtype=torch.float32, device=device)

    # bootstrap value for last obs
    with torch.no_grad():
        _, next_value, _ = model(obs, hidden)
    next_value = next_value.cpu().numpy()

    collect_rollout._hidden = hidden  # carry across rollouts

    # GAE
    adv_buf = np.zeros((T, N), dtype=np.float32)
    lastgaelam = np.zeros(N, dtype=np.float32)
    for t in reversed(range(T)):
        nextnonterminal = 1.0 - done_buf[t]
        nextvalue = next_value if t == T - 1 else val_buf[t + 1]
        delta = rew_buf[t] + cfg.train.gamma * nextvalue * nextnonterminal - val_buf[t]
        lastgaelam = (
            delta
            + cfg.train.gamma * cfg.train.gae_lambda * nextnonterminal * lastgaelam
        )
        adv_buf[t] = lastgaelam
    ret_buf = adv_buf + val_buf

    return {
        "obs": obs_buf,
        "actions": act_buf,
        "logprobs": logp_buf,
        "values": val_buf,
        "advantages": adv_buf,
        "returns": ret_buf,
        "dones": done_buf,
        "rewards": rew_buf,
        "h_starts": h_starts,
        "T": T,
        "N": N,
    }


def ppo_update(model, opt, buf, cfg: Config, device: str) -> Dict[str, float]:
    T, N = buf["T"], buf["N"]
    tc = cfg.train
    stats = {"pg_loss": 0.0, "v_loss": 0.0, "entropy": 0.0, "approx_kl": 0.0, "n": 0}

    if not model.recurrent:
        b_obs = torch.as_tensor(buf["obs"].reshape(T * N, -1), device=device)
        b_act = torch.as_tensor(buf["actions"].reshape(-1), device=device)
        b_logp = torch.as_tensor(buf["logprobs"].reshape(-1), device=device)
        b_adv = torch.as_tensor(buf["advantages"].reshape(-1), device=device)
        b_ret = torch.as_tensor(buf["returns"].reshape(-1), device=device)
        b_val = torch.as_tensor(buf["values"].reshape(-1), device=device)
        n = T * N
        idx = np.arange(n)
        for _ in range(tc.ppo_epochs):
            np.random.shuffle(idx)
            for start in range(0, n, tc.minibatch_size):
                mb = idx[start : start + tc.minibatch_size]
                logits, value, _ = model(b_obs[mb])
                dist = torch.distributions.Categorical(logits=logits)
                newlogp = dist.log_prob(b_act[mb])
                entropy = dist.entropy().mean()
                logratio = newlogp - b_logp[mb]
                ratio = logratio.exp()
                adv = b_adv[mb]
                adv = (adv - adv.mean()) / (adv.std() + 1e-8)
                pg1 = -adv * ratio
                pg2 = -adv * torch.clamp(ratio, 1 - tc.clip_coef, 1 + tc.clip_coef)
                pg_loss = torch.max(pg1, pg2).mean()
                v_clip = b_val[mb] + torch.clamp(
                    value - b_val[mb], -tc.clip_coef, tc.clip_coef
                )
                v_loss = 0.5 * torch.max(
                    (value - b_ret[mb]) ** 2, (v_clip - b_ret[mb]) ** 2
                ).mean()
                loss = pg_loss - tc.ent_coef * entropy + tc.vf_coef * v_loss
                opt.zero_grad()
                loss.backward()
                nn.utils.clip_grad_norm_(model.parameters(), tc.max_grad_norm)
                opt.step()
                with torch.no_grad():
                    kl = ((ratio - 1) - logratio).mean().item()
                for k, v in (("pg_loss", pg_loss.item()), ("v_loss", v_loss.item()),
                             ("entropy", entropy.item()), ("approx_kl", kl)):
                    stats[k] += v
                stats["n"] += 1
    else:
        # recurrent: minibatch over (env, chunk) segments, BPTT within chunk
        n_chunks = T // CHUNK_LEN
        segments = [(e, c) for e in range(N) for c in range(n_chunks)]
        segs_per_mb = max(1, tc.minibatch_size // CHUNK_LEN)
        for _ in range(tc.ppo_epochs):
            np.random.shuffle(segments)
            for start in range(0, len(segments), segs_per_mb):
                mb = segments[start : start + segs_per_mb]
                B = len(mb)
                h = torch.cat(
                    [buf["h_starts"][c][:, e : e + 1] for e, c in mb], dim=1
                ).detach()  # (1, B, H)
                pg_losses, v_losses, ents, kls = [], [], [], []
                for t in range(CHUNK_LEN):
                    obs_t = torch.as_tensor(
                        np.stack(
                            [buf["obs"][c * CHUNK_LEN + t, e] for e, c in mb]
                        ),
                        device=device,
                    )
                    act_t = torch.as_tensor(
                        np.array(
                            [buf["actions"][c * CHUNK_LEN + t, e] for e, c in mb]
                        ),
                        device=device,
                    )
                    logp_t = torch.as_tensor(
                        np.array(
                            [buf["logprobs"][c * CHUNK_LEN + t, e] for e, c in mb]
                        ),
                        device=device,
                    )
                    adv_t = torch.as_tensor(
                        np.array(
                            [buf["advantages"][c * CHUNK_LEN + t, e] for e, c in mb]
                        ),
                        device=device,
                    )
                    ret_t = torch.as_tensor(
                        np.array(
                            [buf["returns"][c * CHUNK_LEN + t, e] for e, c in mb]
                        ),
                        device=device,
                    )
                    val_t = torch.as_tensor(
                        np.array(
                            [buf["values"][c * CHUNK_LEN + t, e] for e, c in mb]
                        ),
                        device=device,
                    )
                    # mask hidden at episode boundaries (done at t-1)
                    if t > 0:
                        dprev = torch.as_tensor(
                            np.array(
                                [
                                    buf["dones"][c * CHUNK_LEN + t - 1, e]
                                    for e, c in mb
                                ]
                            ),
                            dtype=torch.float32,
                            device=device,
                        )
                        h = h * (1.0 - dprev).view(1, B, 1)
                    logits, value, h = model(obs_t, h)
                    dist = torch.distributions.Categorical(logits=logits)
                    newlogp = dist.log_prob(act_t)
                    entropy = dist.entropy().mean()
                    logratio = newlogp - logp_t
                    ratio = logratio.exp()
                    adv = (adv_t - adv_t.mean()) / (adv_t.std() + 1e-8)
                    pg1 = -adv * ratio
                    pg2 = -adv * torch.clamp(
                        ratio, 1 - tc.clip_coef, 1 + tc.clip_coef
                    )
                    pg_losses.append(torch.max(pg1, pg2).mean())
                    v_clip = val_t + torch.clamp(
                        value - val_t, -tc.clip_coef, tc.clip_coef
                    )
                    v_losses.append(
                        0.5
                        * torch.max((value - ret_t) ** 2, (v_clip - ret_t) ** 2).mean()
                    )
                    ents.append(entropy)
                    with torch.no_grad():
                        kls.append(((ratio - 1) - logratio).mean().item())
                pg_loss = torch.stack(pg_losses).mean()
                v_loss = torch.stack(v_losses).mean()
                entropy = torch.stack(ents).mean()
                loss = pg_loss - tc.ent_coef * entropy + tc.vf_coef * v_loss
                opt.zero_grad()
                loss.backward()
                nn.utils.clip_grad_norm_(model.parameters(), tc.max_grad_norm)
                opt.step()
                stats["pg_loss"] += pg_loss.item()
                stats["v_loss"] += v_loss.item()
                stats["entropy"] += entropy.item()
                stats["approx_kl"] += float(np.mean(kls))
                stats["n"] += 1

    for k in ("pg_loss", "v_loss", "entropy", "approx_kl"):
        stats[k] /= max(stats["n"], 1)
    return stats


# ---------------------------------------------------------------------------
# Training driver
# ---------------------------------------------------------------------------


def train(cfg: Config, out_dir: Path) -> Dict:
    device = cfg.train.device
    torch.manual_seed(cfg.seed)
    np.random.seed(cfg.seed)

    model = build_model(cfg.agent).to(device)
    opt = torch.optim.Adam(model.parameters(), lr=cfg.train.lr)
    vec = VecEnvs(cfg, cfg.train.num_envs, cfg.seed)

    out_dir.mkdir(parents=True, exist_ok=True)
    save_config(cfg, out_dir / "config.yaml")

    hashes: Dict[str, str] = {}
    hashes["pre"] = save_checkpoint(
        model, out_dir / "pre.pt", step=0, seed=cfg.seed, agent=vars(cfg.agent)
    )

    # pre-PPO eval
    policy = TorchPolicy(model, deterministic=True, device=device)
    pre_eval = evaluate(cfg, policy, cfg.train.eval_episodes, seed=cfg.seed + 500)

    n_updates = cfg.train.total_steps // cfg.train.rollout_steps
    best_err = float("inf")
    log: List[Dict] = []
    eval_hist: List[Dict] = []
    t0 = time.time()

    for update in range(1, n_updates + 1):
        buf = collect_rollout(model, vec, cfg, device)
        stats = ppo_update(model, opt, buf, cfg, device)
        stats["update"] = update
        stats["steps"] = update * cfg.train.rollout_steps
        stats["mean_step_reward"] = float(buf["rewards"].mean())
        log.append(stats)

        if update % cfg.train.eval_every_updates == 0 or update == n_updates:
            policy = TorchPolicy(model, deterministic=True, device=device)
            ev = evaluate(
                cfg, policy, cfg.train.eval_episodes, seed=cfg.seed + 500
            )
            err = ev["aggregate"]["homeostatic_error_full"]["mean"]
            eval_hist.append(
                {
                    "update": update,
                    "steps": stats["steps"],
                    "homeostatic_error": ev["aggregate"]["homeostatic_error"]["mean"],
                    "homeostatic_error_full": err,
                    "survival_fraction": ev["aggregate"]["survival_fraction"]["mean"],
                    "stable_fraction": ev["aggregate"]["stable_fraction"]["mean"],
                    "death_rate": ev["aggregate"]["death_rate"],
                }
            )
            if err < best_err:
                best_err = err
                hashes["best"] = save_checkpoint(
                    model, out_dir / "best.pt",
                    step=stats["steps"], seed=cfg.seed,
                    agent=vars(cfg.agent), eval_error=err,
                )
            print(
                f"  upd {update}/{n_updates} err={err:.4f} "
                f"surv={eval_hist[-1]['survival_fraction']:.2f} "
                f"kl={stats['approx_kl']:.4f} ent={stats['entropy']:.3f}",
                flush=True,
            )

    hashes["final"] = save_checkpoint(
        model, out_dir / "final.pt",
        step=cfg.train.total_steps, seed=cfg.seed, agent=vars(cfg.agent),
    )

    wall = time.time() - t0
    meta = {
        "seed": cfg.seed,
        "git_commit": git_commit(),
        "config": cfg.name,
        "agent": vars(cfg.agent),
        "device": device,
        "torch_version": torch.__version__,
        "wall_time_s": round(wall, 2),
        "training_steps": cfg.train.total_steps,
        "n_updates": n_updates,
        "checkpoint_hashes": hashes,
        "pre_eval": pre_eval["aggregate"],
        "best_eval_error": best_err,
    }
    (out_dir / "meta.json").write_text(json.dumps(meta, indent=2))
    (out_dir / "train_log.json").write_text(
        json.dumps({"updates": log, "evals": eval_hist}, indent=2)
    )
    return meta


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--agent", default=None, choices=["mlp", "gru"])
    ap.add_argument("--hidden", type=int, default=None)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--steps", type=int, default=None)
    ap.add_argument("--out", required=True)
    args = ap.parse_args()

    cfg = load_config(args.config)
    if args.agent:
        cfg.agent.kind = args.agent
    if args.hidden:
        cfg.agent.hidden = args.hidden
    if args.seed is not None:
        cfg.seed = args.seed
    if args.steps:
        cfg.train.total_steps = args.steps

    meta = train(cfg, Path(args.out))
    print(json.dumps({"best_eval_error": meta["best_eval_error"],
                      "wall_time_s": meta["wall_time_s"]}, indent=2))


if __name__ == "__main__":
    main()
