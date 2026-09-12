"""P0 trainer: PPO over belief+surprise inputs with an online predictor.

The learned agent chooses among IGNORE / OBSERVE_{A..D} / OBSERVE_ALL.
A ChannelPredictor is trained online inside the rollout loop (masked MSE
on freshly observed channels only) and supplies the surprise features e,d.

GRU policies use the stored-hidden approximation (each step's GRU input
hidden is stored at collection time and replayed detached in the update --
no multi-step BPTT). MLP policies are memoryless.

Run:
    python -m experiments.p0.train --config experiments/p0/configs/base.yaml
Smoke (<60s):
    P0_SMOKE=1 python -m experiments.p0.train --config experiments/p0/configs/base.yaml
"""

from __future__ import annotations

import argparse
import copy
import dataclasses
import json
import os
import subprocess
import time
from pathlib import Path
from typing import Dict, Optional

import numpy as np
import torch
import torch.nn as nn

from .agents.harness import BeliefHarness
from .config import Config, load_config, save_config
from .env import dynamics as dyn
from .env.attention_env import AttentionEnv
from .evaluate import MaskedPolicy, evaluate
from .models.policies import build_model
from .models.predictor import ChannelPredictor


# ---------------------------------------------------------------------------
# Vectorized env
# ---------------------------------------------------------------------------


class VecEnv:
    """Synchronous list of AttentionEnvs with auto-reset on done."""

    def __init__(self, params: dyn.EnvParams, num_envs: int, base_seed: int):
        self.envs = [AttentionEnv(params, seed=base_seed + i)
                     for i in range(num_envs)]
        self._ctr = base_seed + num_envs

    def reset_all(self) -> np.ndarray:
        return np.stack([e.reset() for e in self.envs])

    def step(self, actions: np.ndarray):
        obs, rews, dones, infos = [], [], [], []
        for e, a in zip(self.envs, actions):
            o, r, d, info = e.step(int(a))
            if d:
                e.reseed(self._ctr)
                self._ctr += 1
                o = e._obs()
            obs.append(o)
            rews.append(r)
            dones.append(d)
            infos.append(info)
        return (np.stack(obs), np.array(rews, dtype=np.float64),
                np.array(dones, dtype=bool), infos)


# ---------------------------------------------------------------------------
# PPO
# ---------------------------------------------------------------------------


def git_commit() -> str:
    try:
        return subprocess.check_output(
            ["git", "rev-parse", "--short", "HEAD"],
            cwd=Path(__file__).resolve().parents[2],
            stderr=subprocess.DEVNULL,
        ).decode().strip()
    except Exception:
        return "unknown"


def train(cfg: Config, run_dir: Path, budget_steps: Optional[int] = None) -> Dict:
    t_cfg = cfg.train
    total_steps = budget_steps or t_cfg.total_steps
    device = t_cfg.device
    torch.manual_seed(cfg.seed)
    np.random.seed(cfg.seed)

    model = build_model(cfg.agent).to(device)
    pred = ChannelPredictor(hidden=cfg.predictor.hidden, lr=cfg.predictor.lr,
                            ema=cfg.predictor.ema, seed=cfg.seed + 31337)
    harness = BeliefHarness(pred, err_scale=cfg.predictor.err_scale)
    opt = torch.optim.Adam(model.parameters(), lr=t_cfg.lr, eps=1e-5)

    N, T = t_cfg.num_envs, t_cfg.rollout_steps
    OBS_DIM = dyn.N_CHANNELS * AttentionEnv.FEATS_PER_CHANNEL
    vec = VecEnv(cfg.env, N, base_seed=cfg.seed * 10_000)
    obs_env = vec.reset_all()
    harness.reset(N)
    hidden = None  # (1, N, H) for GRU

    n_updates = max(1, total_steps // (N * T))
    log = {"updates": [], "steps": 0}
    best_eval = -np.inf
    t0 = time.time()

    run_dir.mkdir(parents=True, exist_ok=True)
    save_config(cfg, run_dir / "config.yaml")
    metrics_path = run_dir / "metrics.jsonl"
    # "w", not "a": a re-run must produce a fresh metrics file rather than
    # silently appending duplicate update rows (M1a double-append bug).
    mf = open(metrics_path, "w")

    def save_ckpt(name: str, step: int):
        ckpt = {
            "model": model.state_dict(),
            "predictor": pred.state_dict(),
            "predictor_opt": pred.opt.state_dict(),
            "agent": dataclasses.asdict(cfg.agent),
            "predictor_cfg": dataclasses.asdict(cfg.predictor),
            "step": step,
            "seed": cfg.seed,
        }
        torch.save(ckpt, run_dir / name)

    for update in range(1, n_updates + 1):
        # Linear LR anneal to 0 (standard PPO practice): reduces the
        # late-training instability where several seeds' eval detection
        # collapsed toward IGNORE (M1b).
        lr_now = t_cfg.lr * (1.0 - (update - 1) / n_updates)
        for g in opt.param_groups:
            g["lr"] = lr_now

        b_obs = np.zeros((T, N, OBS_DIM), dtype=np.float64)
        b_act = np.zeros((T, N), dtype=np.int64)
        b_logp = np.zeros((T, N), dtype=np.float64)
        b_rew = np.zeros((T, N), dtype=np.float64)
        b_done = np.zeros((T, N), dtype=np.float64)
        b_val = np.zeros((T, N), dtype=np.float64)
        b_hid = [] if model.recurrent else None
        pred_loss_acc, pred_steps = 0.0, 0

        for t in range(T):
            agent_obs = harness.agent_input(obs_env)
            if cfg.agent.pe_mask:
                # belief-only control: the agent never sees e,d (M2). The
                # predictor still trains normally; only the policy input is
                # masked, and the masked obs is what PPO trains on.
                agent_obs = AttentionEnv.mask_surprise(agent_obs)
            x = torch.as_tensor(agent_obs, dtype=torch.float32, device=device)
            with torch.no_grad():
                logits, value, new_hidden = model.forward(x, hidden)
                dist = torch.distributions.Categorical(logits=logits)
                act = dist.sample()
                logp = dist.log_prob(act)
            b_obs[t] = agent_obs
            b_act[t] = act.cpu().numpy()
            b_logp[t] = logp.cpu().numpy()
            b_val[t] = value.cpu().numpy()
            if model.recurrent:
                h0 = (torch.zeros(1, N, model.hidden_size)
                      if hidden is None else hidden)
                b_hid.append(h0.squeeze(0).cpu().numpy().copy())

            obs_env, rew, done, _info = vec.step(b_act[t])
            pl, _ = harness.update(obs_env, just_reset=done)
            pred_loss_acc += pl
            pred_steps += 1
            b_rew[t] = rew
            b_done[t] = done.astype(np.float64)
            hidden = new_hidden
            if model.recurrent and done.any():
                hidden = hidden.clone()
                hidden[0, done] = 0.0

        # ---- GAE ----
        with torch.no_grad():
            last_obs = harness.agent_input(obs_env)
            if cfg.agent.pe_mask:
                last_obs = AttentionEnv.mask_surprise(last_obs)
            x_last = torch.as_tensor(last_obs, dtype=torch.float32, device=device)
            _, last_val, _ = model.forward(x_last, hidden)
            last_val = last_val.cpu().numpy()
        adv = np.zeros((T, N), dtype=np.float64)
        lastgae = np.zeros(N, dtype=np.float64)
        for t in reversed(range(T)):
            nonterminal = 1.0 - b_done[t]
            nextval = last_val if t == T - 1 else b_val[t + 1]
            delta = b_rew[t] + t_cfg.gamma * nextval * nonterminal - b_val[t]
            lastgae = delta + t_cfg.gamma * t_cfg.gae_lambda * nonterminal * lastgae
            adv[t] = lastgae
        ret = adv + b_val

        # ---- PPO update (flattened minibatches; GRU: stored-hidden approx) ----
        f_obs = torch.as_tensor(b_obs.reshape(T * N, OBS_DIM), dtype=torch.float32)
        f_act = torch.as_tensor(b_act.reshape(T * N))
        f_logp = torch.as_tensor(b_logp.reshape(T * N), dtype=torch.float32)
        f_adv = torch.as_tensor(adv.reshape(T * N), dtype=torch.float32)
        f_ret = torch.as_tensor(ret.reshape(T * N), dtype=torch.float32)
        f_hid = (torch.as_tensor(np.stack(b_hid).reshape(T * N, -1),
                                 dtype=torch.float32)
                 if model.recurrent else None)
        f_adv = (f_adv - f_adv.mean()) / (f_adv.std() + 1e-8)

        idx = np.arange(T * N)
        clipfracs, approx_kl = [], []
        stop = False
        for _ in range(t_cfg.ppo_epochs):
            np.random.shuffle(idx)
            for start in range(0, T * N, t_cfg.minibatch_size):
                mb = idx[start:start + t_cfg.minibatch_size]
                mb_obs = f_obs[mb].to(device)
                if model.recurrent:
                    mb_h = f_hid[mb].to(device).unsqueeze(0)  # (1, mb, H)
                else:
                    mb_h = None
                logits, value, _ = model.forward(mb_obs, mb_h)
                dist = torch.distributions.Categorical(logits=logits)
                newlogp = dist.log_prob(f_act[mb].to(device))
                ratio = (newlogp - f_logp[mb].to(device)).exp()
                madv = f_adv[mb].to(device)
                pg1 = -madv * ratio
                pg2 = -madv * torch.clamp(ratio, 1 - t_cfg.clip_coef,
                                          1 + t_cfg.clip_coef)
                pg_loss = torch.max(pg1, pg2).mean()
                v_loss = 0.5 * ((value - f_ret[mb].to(device)) ** 2).mean()
                ent = dist.entropy().mean()
                loss = pg_loss - t_cfg.ent_coef * ent + t_cfg.vf_coef * v_loss
                opt.zero_grad()
                loss.backward()
                nn.utils.clip_grad_norm_(model.parameters(), t_cfg.max_grad_norm)
                opt.step()
                with torch.no_grad():
                    clipfracs.append(((ratio - 1).abs() > t_cfg.clip_coef)
                                     .float().mean().item())
                    kl = (ratio - 1 - ratio.log()).mean().item()
                    approx_kl.append(kl)
                if t_cfg.target_kl is not None and kl > t_cfg.target_kl:
                    stop = True
                    break
            if stop:
                break

        steps_done = update * T * N
        row = {
            "update": update, "steps": steps_done,
            "reward_per_step": float(b_rew.mean()),
            "pred_loss": pred_loss_acc / max(pred_steps, 1),
            "clipfrac": float(np.mean(clipfracs)),
            "approx_kl": float(np.mean(approx_kl)),
            "elapsed_s": round(time.time() - t0, 1),
        }

        # ---- periodic eval ----
        if update % t_cfg.eval_every_updates == 0 or update == n_updates:
            eval_pred = copy.deepcopy(pred)
            eval_pol = DeterministicPolicy(model, device)
            if cfg.agent.pe_mask:
                eval_pol = MaskedPolicy(eval_pol)
            res = evaluate(cfg, eval_pol,
                           cfg.train.eval_episodes, seed=cfg.seed + 5_000,
                           predictor=eval_pred)
            agg = res["aggregate"]
            row["eval_reward"] = agg["total_reward"]["mean"]
            row["eval_detection"] = agg["detection_rate_pooled"]
            row["eval_cost"] = agg["mean_cost"]["mean"]
            row["eval_efficiency"] = agg["info_efficiency"]["mean"]
            if not np.isnan(agg["total_reward"]["mean"]) and \
                    agg["total_reward"]["mean"] > best_eval:
                best_eval = agg["total_reward"]["mean"]
                save_ckpt("ckpt_best.pt", steps_done)
        mf.write(json.dumps(row) + "\n")
        mf.flush()
        if update % max(1, n_updates // 10) == 0 or update == 1:
            print(f"[{steps_done}/{total_steps}] " +
                  " ".join(f"{k}={v:.4g}" if isinstance(v, float) else f"{k}={v}"
                           for k, v in row.items() if k != "update"))

    save_ckpt("ckpt_final.pt", total_steps)
    mf.close()

    meta = {
        "name": cfg.name, "seed": cfg.seed, "git": git_commit(),
        "config": dataclasses.asdict(cfg.train) | {
            "env": dataclasses.asdict(cfg.env),
            "agent": dataclasses.asdict(cfg.agent),
            "predictor": dataclasses.asdict(cfg.predictor)},
        "device": device, "total_steps": int(total_steps),
        "wall_time_s": round(time.time() - t0, 1),
        "ckpt_best": "ckpt_best.pt",
        "ckpt_final": "ckpt_final.pt",
        "best_eval_reward": best_eval,
    }
    (run_dir / "meta.json").write_text(json.dumps(meta, indent=2))
    return meta


class DeterministicPolicy:
    """Argmax policy wrapper used for periodic evals."""

    def __init__(self, model, device="cpu"):
        self.model = model
        self.device = device
        self.hidden = None

    def reset(self):
        self.hidden = None

    def act(self, obs: np.ndarray) -> int:
        with torch.no_grad():
            x = torch.as_tensor(obs, dtype=torch.float32, device=self.device)
            logits, self.hidden = self.model.policy_logits(x, self.hidden)
            return int(logits.argmax(-1).item())


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--total-steps", type=int, default=None)
    ap.add_argument("--num-envs", type=int, default=None)
    ap.add_argument("--out", default=None, help="run dir override")
    args = ap.parse_args()

    cfg = load_config(args.config)
    if args.seed is not None:
        cfg.seed = args.seed
    if args.total_steps is not None:
        cfg.train.total_steps = args.total_steps
    if args.num_envs is not None:
        cfg.train.num_envs = args.num_envs

    smoke = os.environ.get("P0_SMOKE", "") == "1"
    if smoke:
        cfg.train.total_steps = int(os.environ.get("P0_TOTAL_STEPS", 12_000))
        cfg.train.rollout_steps = 64
        cfg.train.num_envs = int(os.environ.get("P0_NUM_ENVS", 4))
        cfg.train.eval_every_updates = 3
        cfg.train.eval_episodes = 2
        cfg.eval.episodes = int(os.environ.get("P0_EVAL_EPISODES", 4))

    run_dir = Path(args.out) if args.out else (
        Path(__file__).resolve().parent / "runs"
        / f"{cfg.name}_{cfg.agent.kind}_s{cfg.seed}"
    )
    meta = train(cfg, run_dir)
    print(json.dumps({k: v for k, v in meta.items() if k != "config"}, indent=2))


if __name__ == "__main__":
    main()
