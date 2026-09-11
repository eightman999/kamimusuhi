"""H0 evaluation runner.

Works for any object with reset() / act(obs)->int (random, heuristic, or a
torch policy wrapper). Supports the causal ablations from spec section 13:

    ablation = {"type": "none"}
    ablation = {"type": "shuffle"}                 # C1 internal shuffle
    ablation = {"type": "mask", "index": 0, "value": 0.5}   # C2 internal mask
    ablation = {"type": "permute", "permutation": [...]}    # C3 sensor perm
    ablation = {"type": "hidden_reset", "at": 0.5}          # reset h mid-ep
    ablation = {"type": "action_permutation", ...}          # C4 via EnvParams

C4 is applied through EnvParams.action_permutation, not here.

CLI:
    python -m experiments.h0.evaluate --config configs/base.yaml \
        --agent heuristic --episodes 20 --out results/heuristic.json
"""

from __future__ import annotations

import argparse
import json
import time
from pathlib import Path
from typing import Callable, Dict, List, Optional

import numpy as np

from .analysis import metrics
from .config import Config, load_config
from .env import dynamics as dyn
from .env.homeostasis_env import HomeostasisEnv


# ---------------------------------------------------------------------------
# Episode runner
# ---------------------------------------------------------------------------


def run_episode(
    env: HomeostasisEnv,
    policy,
    rng: np.random.Generator,
    ablation: Optional[Dict] = None,
    donor_pool: Optional[np.ndarray] = None,
    collect_trace: bool = True,
) -> Dict:
    """Run one episode. Returns per-episode metrics (+ optional trace)."""
    ablation = ablation or {"type": "none"}
    obs = env.reset()
    policy.reset()
    internals, actions, rewards = [], [], []
    hidden_reset_at = None
    if ablation.get("type") == "hidden_reset":
        hidden_reset_at = int(env.params.episode_length * ablation.get("at", 0.5))

    done = False
    while not done:
        internals.append(env.internal.copy())
        obs_in = obs
        t = ablation.get("type")
        if t == "shuffle" and donor_pool is not None:
            donor = donor_pool[rng.integers(len(donor_pool))]
            obs_in = HomeostasisEnv.shuffle_internal(obs, donor)
        elif t == "mask":
            obs_in = HomeostasisEnv.mask_internal(
                obs, ablation["index"], ablation.get("value", 0.5)
            )
        elif t == "permute":
            obs_in = HomeostasisEnv.permute_obs(obs, np.asarray(ablation["permutation"]))
        if hidden_reset_at is not None and env.t == hidden_reset_at:
            policy.reset()

        a = policy.act(obs_in)
        actions.append(int(a))
        obs, r, done, info = env.step(int(a))
        rewards.append(r)

    m = metrics.episode_metrics(
        np.array(internals),
        np.array(actions),
        np.array(rewards),
        survived_steps=env.t,
        episode_length=env.params.episode_length,
        death_cause=env.death_cause,
    )
    if collect_trace:
        m["trace"] = {
            "internal": np.array(internals).tolist(),
            "action": [int(a) for a in actions],
            "reward": [float(r) for r in rewards],
        }
    return m


def build_donor_pool(
    env: HomeostasisEnv,
    policy,
    n_episodes: int,
    rng: np.random.Generator,
    base_seed: int,
) -> np.ndarray:
    """Collect internal states from other episodes (C1 donor pool)."""
    pool = []
    for i in range(n_episodes):
        env.reseed(base_seed + 10_000 + i)
        obs = env.reset()
        policy.reset()
        done = False
        while not done:
            pool.append(env.internal.copy())
            obs, _, done, _ = env.step(policy.act(obs))
    return np.array(pool)


def evaluate(
    cfg: Config,
    policy,
    n_episodes: int,
    seed: int = 0,
    ablation: Optional[Dict] = None,
    collect_traces: bool = False,
) -> Dict:
    """Evaluate a policy over n_episodes; returns aggregate + episodes."""
    rng = np.random.default_rng(seed + 777)
    env = HomeostasisEnv(cfg.env, seed=seed)
    ablation = ablation or {"type": "none"}

    donor_pool = None
    if ablation.get("type") == "shuffle":
        donor_pool = build_donor_pool(env, policy, 3, rng, seed)

    episodes = []
    for i in range(n_episodes):
        env.reseed(seed * 1000 + i)
        ep = run_episode(
            env, policy, rng, ablation=ablation,
            donor_pool=donor_pool, collect_trace=collect_traces,
        )
        episodes.append(ep)

    return {"aggregate": metrics.aggregate(episodes), "episodes": episodes}


# ---------------------------------------------------------------------------
# Policy wrapper for torch models
# ---------------------------------------------------------------------------


class TorchPolicy:
    """Wraps an actor-critic nn.Module as a reset()/act() policy."""

    def __init__(self, model, deterministic: bool = True, device: str = "cpu"):
        import torch

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


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def make_policy(cfg: Config, checkpoint: Optional[str] = None):
    """Build a policy from cfg.agent; loads torch checkpoint if given."""
    kind = cfg.agent.kind
    if kind == "random":
        from .agents.random_agent import RandomAgent

        return RandomAgent(seed=cfg.seed)
    if kind == "heuristic":
        from .agents.heuristic import HeuristicAgent

        return HeuristicAgent()
    if kind in ("mlp", "gru"):
        from .agents.models import build_model

        model = build_model(cfg.agent)
        if checkpoint:
            import torch

            state = torch.load(checkpoint, map_location="cpu", weights_only=True)
            model.load_state_dict(state["model"])
        model.eval()
        return TorchPolicy(model)
    raise ValueError(f"unknown agent kind {kind}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--config", required=True)
    ap.add_argument("--agent", default=None, help="override agent kind")
    ap.add_argument("--checkpoint", default=None)
    ap.add_argument("--episodes", type=int, default=None)
    ap.add_argument("--seed", type=int, default=None)
    ap.add_argument("--ablation", default="none")
    ap.add_argument("--mask-index", type=int, default=0)
    ap.add_argument("--traces", action="store_true")
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
    if args.ablation == "mask":
        ablation.update({"index": args.mask_index, "value": 0.5})
    elif args.ablation == "permute":
        rng = np.random.default_rng(123)
        ablation["permutation"] = rng.permutation(dyn.OBS_DIM).tolist()
    elif args.ablation == "hidden_reset":
        ablation["at"] = 0.5

    policy = make_policy(cfg, args.checkpoint)
    t0 = time.time()
    result = evaluate(
        cfg, policy, cfg.eval.episodes, seed=cfg.seed,
        ablation=ablation, collect_traces=args.traces,
    )
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
