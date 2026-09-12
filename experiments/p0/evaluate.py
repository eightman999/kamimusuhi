"""P0 evaluation runner.

Works for any object with reset() / act(obs_24)->int. Ablations act on the
agent input (belief+surprise vector); the env and predictor are untouched:

    {"type": "none"}
    {"type": "pe_shuffle", "permutation": [..C..]}   # permute e,d across channels
    {"type": "pe_mask"}                              # zero all e,d features
    {"type": "ch_permute", "permutation": [..C..]}   # permute channel blocks
    {"type": "mask_belief", "channel": 3}            # zero belief feats of ch

Env-side interventions (event-frequency shift, cost shift, OOD) are applied
through EnvParams in the config, not here.

CLI:
    python -m experiments.p0.evaluate --config experiments/p0/configs/base.yaml \
        --agent fixed --episodes 12 --out reports/eval_fixed.json
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
from typing import Dict, Optional

import numpy as np

from .agents.harness import BeliefHarness
from .analysis import metrics
from .config import Config, load_config
from .env import dynamics as dyn
from .env.attention_env import AttentionEnv
from .models.predictor import ChannelPredictor


def apply_ablation(agent_obs: np.ndarray, ablation: Dict) -> np.ndarray:
    t = ablation.get("type", "none")
    if t == "none":
        return agent_obs
    if t == "pe_shuffle":
        return AttentionEnv.shuffle_surprise(
            agent_obs, np.asarray(ablation["permutation"])
        )
    if t == "pe_mask":
        return AttentionEnv.mask_surprise(agent_obs)
    if t == "pe_mask_ch":
        return AttentionEnv.mask_surprise_ch(agent_obs, ablation["channel"])
    if t == "ch_permute":
        return AttentionEnv.permute_channels(
            agent_obs, np.asarray(ablation["permutation"])
        )
    if t == "mask_belief":
        return AttentionEnv.mask_belief(agent_obs, ablation["channel"])
    raise ValueError(f"unknown ablation {t}")


# Eval episodes draw env seeds from a dedicated range so they can never
# collide with training episode seeds (VecEnv uses cfg.seed*10_000 + ctr).
EVAL_SEED_BASE = 500_000


def run_episode(
    env: AttentionEnv,
    policy,
    harness: BeliefHarness,
    ablation: Optional[Dict] = None,
    pred_train: bool = True,
) -> Dict:
    ablation = ablation or {"type": "none"}
    obs_env = env.reset()
    policy.reset()
    harness.reset(1)

    actions, rewards, costs = [], [], []
    surprise_acc = np.zeros(dyn.N_CHANNELS)
    pred_err_acc = np.zeros(dyn.N_CHANNELS)
    pred_err_cnt = np.zeros(dyn.N_CHANNELS)
    event_steps = 0

    done = False
    while not done:
        inp = harness.agent_input(obs_env[None, :])          # (1, 24)
        inp = apply_ablation(inp[0], ablation)
        a = policy.act(inp)
        surprise_acc += harness.surprise(obs_env[None, :])[0]

        obs_env, r, done, info = env.step(a)
        # predictor trains on freshly observed channels only; per-channel
        # raw |xhat - v| recorded for metrics
        mask = obs_env[4:8] == 0.0                            # staleness == 0
        _, err = harness.update(obs_env[None, :], train=pred_train)
        pred_err_acc += err[0] * mask
        pred_err_cnt += mask

        actions.append(int(a))
        rewards.append(r)
        costs.append(info["cost"])
        event_steps += int(info["event_active"])

    pred_err_mean = np.divide(
        pred_err_acc, np.maximum(pred_err_cnt, 1),
        out=np.zeros_like(pred_err_acc), where=pred_err_cnt > 0,
    )
    T = max(len(actions), 1)
    return metrics.episode_metrics(
        np.array(actions), np.array(rewards), np.array(costs),
        n_detected=env.d.n_detected, n_missed=env.d.n_missed,
        surprise_mean=surprise_acc / T, pred_err_mean=pred_err_mean,
        event_active_steps=event_steps,
    )


def evaluate(
    cfg: Config,
    policy,
    n_episodes: int,
    seed: int = 0,
    ablation: Optional[Dict] = None,
    predictor: Optional[ChannelPredictor] = None,
    pred_train: bool = True,
) -> Dict:
    """Run `n_episodes` eval episodes.

    pred_train: if False the predictor's weights are frozen during eval (no
    SGD; the EMA error feature still updates). The default True preserves the
    training-time semantics where the predictor keeps learning from whatever
    the policy chooses to observe -- which also means eval episodes are NOT
    iid (the predictor drifts across them) and ablations couple back into the
    predictor through the policy's observation choices.
    """
    env = AttentionEnv(cfg.env, seed=seed)
    # A fresh predictor is seeded from the eval seed: baseline evals are
    # deterministic and comparable across phases (no init-order variance).
    pred = predictor or ChannelPredictor(
        hidden=cfg.predictor.hidden, lr=cfg.predictor.lr,
        ema=cfg.predictor.ema, seed=seed * 7919 + 13,
    )
    harness = BeliefHarness(pred, err_scale=cfg.predictor.err_scale)
    episodes = []
    for i in range(n_episodes):
        env.reseed(EVAL_SEED_BASE + seed * 1000 + i)
        episodes.append(run_episode(env, policy, harness, ablation=ablation,
                                    pred_train=pred_train))
    return {"aggregate": metrics.aggregate(episodes), "episodes": episodes,
            "predictor": pred}


# ---------------------------------------------------------------------------
# Policy wrapper for torch models
# ---------------------------------------------------------------------------


class TorchPolicy:
    """Wraps an actor-critic nn.Module as a reset()/act() policy."""

    def __init__(self, model, deterministic: bool = True, device: str = "cpu"):
        self.model = model
        self.deterministic = deterministic
        self.device = device
        self.hidden = None

    def reset(self):
        self.hidden = None

    def act(self, obs: np.ndarray) -> int:
        import torch

        with torch.no_grad():
            x = torch.as_tensor(obs, dtype=torch.float32, device=self.device)
            logits, self.hidden = self.model.policy_logits(x, self.hidden)
            if self.deterministic:
                return int(logits.argmax(-1).item())
            dist = torch.distributions.Categorical(logits=logits)
            return int(dist.sample().item())


class MaskedPolicy:
    """Wraps a policy and zeroes the (e, d) surprise features before act().

    The belief-only control: the agent is trained AND evaluated with e,d
    masked, so its input contains only (v, s, pv, g) per channel. Masking is
    a property of the agent, so eval rows still report ablation='none'.
    """

    def __init__(self, inner):
        self.inner = inner
        self.name = getattr(inner, "name", "masked")

    def reset(self):
        self.inner.reset()

    def act(self, obs: np.ndarray) -> int:
        return int(self.inner.act(AttentionEnv.mask_surprise(obs)))


def make_policy(cfg: Config, checkpoint: Optional[str] = None,
                deterministic: bool = True):
    kind = cfg.agent.kind
    if kind in ("random", "never", "roundrobin", "always", "fixed",
                "pe_heuristic", "pe_thresh", "trigger", "vtrigger",
                "vtrigger22", "cwatch") or kind.startswith("clock"):
        from .agents.baselines import make_baseline

        return make_baseline(kind, seed=cfg.seed)
    if kind in ("mlp", "gru"):
        from .models.policies import build_model

        model = build_model(cfg.agent)
        if checkpoint:
            import torch

            state = torch.load(checkpoint, map_location="cpu", weights_only=True)
            model.load_state_dict(state["model"])
        model.eval()
        pol = TorchPolicy(model, deterministic=deterministic)
        if cfg.agent.pe_mask:
            pol = MaskedPolicy(pol)
        return pol
    raise ValueError(f"unknown agent kind {kind}")


def load_predictor(cfg: Config, checkpoint: Optional[str],
                   seed: Optional[int] = None) -> ChannelPredictor:
    """Predictor seeded by `seed` (deterministic init); restores weights and
    Adam moments from checkpoint when present."""
    pred = ChannelPredictor(hidden=cfg.predictor.hidden, lr=cfg.predictor.lr,
                            ema=cfg.predictor.ema,
                            seed=(seed if seed is not None else cfg.seed))
    if checkpoint:
        import torch

        state = torch.load(checkpoint, map_location="cpu", weights_only=True)
        if "predictor" in state:
            pred.load_state_dict(state["predictor"])
        if "predictor_opt" in state:
            pred.opt.load_state_dict(state["predictor_opt"])
    return pred


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--agent", default=None, help="override agent kind")
    ap.add_argument("--checkpoint", default=None)
    ap.add_argument("--episodes", type=int, default=None)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--ablation", default="none")
    ap.add_argument("--mask-channel", type=int, default=dyn.CH_D)
    ap.add_argument("--stochastic", action="store_true",
                    help="sample actions from the policy instead of argmax")
    ap.add_argument("--frozen-pred", action="store_true",
                    help="freeze predictor weights during eval (no SGD)")
    ap.add_argument("--fresh-pred", action="store_true",
                    help="use a fresh predictor instead of the ckpt one")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    cfg = load_config(args.config)
    if args.agent:
        cfg.agent.kind = args.agent
    if args.episodes:
        cfg.eval.episodes = args.episodes
    if args.seed is not None:
        cfg.seed = args.seed

    ablation: Dict = {"type": args.ablation}
    rng = np.random.default_rng(123)
    if args.ablation == "pe_shuffle":
        perm = rng.permutation(dyn.N_CHANNELS)
        while np.all(perm == np.arange(dyn.N_CHANNELS)):
            perm = rng.permutation(dyn.N_CHANNELS)
        ablation["permutation"] = perm.tolist()
    elif args.ablation == "ch_permute":
        perm = rng.permutation(dyn.N_CHANNELS)
        while np.all(perm == np.arange(dyn.N_CHANNELS)):
            perm = rng.permutation(dyn.N_CHANNELS)
        ablation["permutation"] = perm.tolist()
    elif args.ablation == "pe_mask_ch":
        ablation["channel"] = args.mask_channel
    elif args.ablation == "mask_belief":
        ablation["channel"] = args.mask_channel

    policy = make_policy(cfg, args.checkpoint,
                         deterministic=not args.stochastic)
    t0 = time.time()
    pred = None if args.fresh_pred else load_predictor(cfg, args.checkpoint)
    result = evaluate(cfg, policy, cfg.eval.episodes, seed=cfg.seed,
                      ablation=ablation, predictor=pred,
                      pred_train=not args.frozen_pred)
    result.pop("predictor")
    result["wall_time_s"] = round(time.time() - t0, 2)
    result["config"] = args.config
    result["agent"] = cfg.agent.kind
    result["ablation"] = ablation

    text = json.dumps(result, indent=2)
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(text)
        print(f"wrote {args.out}")
    else:
        print(text)


if __name__ == "__main__":
    main()
