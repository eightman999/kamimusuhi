"""Full-sequence imitation and PPO training for T0 temporal tasks.

Conventions follow the K0-E2 pipeline: atomic JSON artifacts, full RNG
snapshots in checkpoints, whole-sequence updates (never shuffle timesteps),
and seed-separated RNG streams for imitation / PPO / validation.
"""
import argparse
import copy
import hashlib
import json
import os
import random
import subprocess
import time
from pathlib import Path

import numpy as np
import torch
from torch import nn

from experiments.t0.env.temporal_tasks import make_task
from experiments.t0.models import make_model


def atomic(path, obj):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temp = path.with_suffix(path.suffix + ".tmp")
    temp.write_text(json.dumps(obj, indent=2, allow_nan=False) + "\n")
    temp.replace(path)


def rng_state(env=None):
    return dict(python=random.getstate(), numpy=np.random.get_state(),
                torch=torch.get_rng_state(),
                cuda=torch.cuda.get_rng_state_all() if torch.cuda.is_available() else None,
                env=env.snapshot() if env else None)


def restore_rng(state, env=None):
    random.setstate(state["python"])
    np.random.set_state(state["numpy"])
    torch.set_rng_state(state["torch"].cpu())
    if state["cuda"] is not None and torch.cuda.is_available():
        torch.cuda.set_rng_state_all([s.cpu() for s in state["cuda"]])
    if env is not None and state["env"] is not None:
        env.restore(state["env"])


def tensor_hash(model):
    h = hashlib.sha256()
    for name, value in model.state_dict().items():
        h.update(name.encode())
        h.update(value.detach().cpu().numpy().tobytes())
    return h.hexdigest()


def save_checkpoint(path, model, optimizer, config, env, stage, update, **extra):
    assert stage in ("initial", "imitation", "ppo")
    p = Path(path)
    tmp = p.with_suffix(".tmp")
    torch.save(dict(model=model.state_dict(), optimizer=optimizer.state_dict(),
                    config=config, rng=rng_state(env), stage=stage, update=update,
                    model_sha256=tensor_hash(model), **extra), tmp)
    tmp.replace(p)


def collect(model, env, teacher_probability=0., greedy=False, intervention=None,
            record_states=False):
    """Rollout to horizon; ``alive`` marks steps before each episode's end.

    ``intervention(t, state, observation)`` may rewrite the hidden state or the
    observation *before* the forward pass at step t (used for T-C1..T-C3).
    Returns column tensors [H, N, ...], a bool mask [H, N], per-step infos and
    optionally the post-forward hidden states [H, N, hidden].
    """
    observation = env.reset()
    state = model.initial_state(env.num_envs, env.device)
    h_steps, n = env.horizon, env.num_envs
    cols = {"obs": [], "actions": [], "logp": [], "values": [], "rewards": [],
            "targets": []}
    mask, infos, states = [], [], []
    alive = torch.ones(n, dtype=torch.bool, device=env.device)
    with torch.no_grad():
        for t in range(h_steps):
            if not bool(alive.any()):
                break
            if intervention is not None:
                state, observation = intervention(t, state, observation)
            logits, value, state = model(observation, state)
            dist = torch.distributions.Categorical(logits=logits)
            target = env.oracle_actions()
            action = logits.argmax(-1) if greedy else dist.sample()
            if teacher_probability:
                teach = torch.rand(n, device=env.device) < teacher_probability
                action = torch.where(teach, target, action)
            action = torch.where(alive, action, torch.zeros_like(action))
            nxt, reward, done, info = env.step(action)
            cols["obs"].append(observation.clone())
            cols["actions"].append(action.clone())
            cols["logp"].append(dist.log_prob(action).clone())
            cols["values"].append(value.clone())
            cols["rewards"].append(reward.clone())
            cols["targets"].append(target.clone())
            mask.append(alive.clone())
            infos.append(info)
            if record_states:
                states.append(state.clone())
            observation = nxt
            alive &= ~done
    length = len(mask)
    rollout = {k: torch.stack(v) for k, v in cols.items()}
    mask = torch.stack(mask)
    result = (rollout, mask, infos)
    if record_states:
        result += (torch.stack(states),)
    return result


def episode_metrics(rollout, mask, infos, env):
    rewards = rollout["rewards"]
    terminal = infos[-1]
    acted = terminal["acted"].float()
    success = terminal["success"].float()
    errors = terminal["timing_error"].float()
    valid = terminal["acted"]
    return dict(success=float(success.mean()), act_rate=float(acted.mean()),
                mean_timing_error=float(errors[valid].mean()) if bool(valid.any()) else None,
                mean_abs_error=float(errors[valid].abs().mean()) if bool(valid.any()) else None,
                early_rate=float(terminal["early"].float().mean()),
                late_rate=float(terminal["late"].float().mean()),
                reward=float((rewards * mask.float()).sum(0).mean()))


def validation(model, device, config, n=256):
    env = make_task(config.get("task", "interval"), n, device, 700001,
                    dict(config.get("env", {}), delay_distribution="grid"))
    rollout, mask, infos = collect(model, env, greedy=True)[:3]
    return episode_metrics(rollout, mask, infos, env)


def advantages(rewards, values, mask, gamma=.99, lam=.95):
    """GAE with per-env termination; dead steps contribute exactly zero."""
    result = torch.zeros_like(rewards)
    carry = torch.zeros_like(rewards[0])
    h = len(rewards)
    for t in reversed(range(h)):
        following = values[t + 1] * mask[t + 1].float() if t + 1 < h else torch.zeros_like(carry)
        delta = (rewards[t] + gamma * following - values[t]) * mask[t].float()
        carry = delta + gamma * lam * carry * mask[t].float()
        result[t] = carry
    return result, result + values


def imitation_update(model, optimizer, rollout, mask, config):
    obs, targets = rollout["obs"], rollout["targets"]
    n_actions = model.num_actions
    freq = torch.bincount(targets[mask].flatten(), minlength=n_actions).float().clamp_min(1)
    weights = (freq.sum() / freq).sqrt()
    weights /= weights.mean()
    logs = []
    for ids in torch.randperm(obs.shape[1], device=obs.device).split(config["minibatch_envs"]):
        logits, _, _ = model.forward_sequence(obs[:, ids],
                                              model.initial_state(len(ids), obs.device))
        loss = nn.functional.cross_entropy(
            logits.flatten(0, 1), targets[:, ids].flatten(),
            weight=weights, reduction="none").view_as(targets[:, ids])
        weight = mask[:, ids].float()
        weight = weight * torch.where(targets[:, ids] > 0, 32., 1.)
        loss = (loss * weight).sum() / weight.sum().clamp_min(1)
        optimizer.zero_grad(set_to_none=True)
        loss.backward()
        norm = nn.utils.clip_grad_norm_(model.parameters(), 1.0)
        optimizer.step()
        logs.append((float(loss.detach()), float(norm)))
    return dict(loss=float(np.mean([x[0] for x in logs])),
                grad_norm=float(np.mean([x[1] for x in logs])))


def ppo_update(model, optimizer, rollout, mask, config):
    obs = rollout["obs"]
    actions, oldlog = rollout["actions"], rollout["logp"]
    oldvalues, rewards = rollout["values"], rollout["rewards"]
    adv, returns = advantages(rewards, oldvalues, mask,
                              config.get("gamma", .99), config.get("gae_lambda", .95))
    adv = (adv - adv[mask].mean()) / (adv[mask].std() + 1e-8)
    clip = config.get("clip_ratio", .2)
    logs = []
    for _ in range(config["ppo_epochs"]):
        for ids in torch.randperm(obs.shape[1], device=obs.device).split(config["minibatch_envs"]):
            logits, values, _ = model.forward_sequence(
                obs[:, ids], model.initial_state(len(ids), obs.device))
            dist = torch.distributions.Categorical(logits=logits)
            m = mask[:, ids]
            ratio = (dist.log_prob(actions[:, ids]) - oldlog[:, ids]).exp()
            a = adv[:, ids]
            policy = -torch.minimum(ratio * a, ratio.clamp(1 - clip, 1 + clip) * a)
            policy = (policy * m.float()).sum() / m.float().sum().clamp_min(1)
            value = (((values - returns[:, ids]).square()) * m.float()).sum() \
                / m.float().sum().clamp_min(1)
            entropy = (dist.entropy() * m.float()).sum() / m.float().sum().clamp_min(1)
            loss = policy + .5 * value - .01 * entropy
            optimizer.zero_grad(set_to_none=True)
            loss.backward()
            norm = nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()
            logs.append([float(x.detach()) for x in (loss, policy, value, entropy, norm)])
    vals = np.mean(logs, axis=0)
    return dict(zip(["loss", "policy_loss", "value_loss", "entropy", "grad_norm"],
                    vals.tolist()))


def train(config, artifacts, run_id, parent=None):
    torch.set_num_threads(int(config.get("threads", 4)))
    seed = config["seed"]
    random.seed(seed)
    np.random.seed(seed)
    torch.manual_seed(seed)
    device = config.get("device", "cpu")
    directory = Path(artifacts) / "runs" / run_id
    if (directory / "status.json").exists():
        raise ValueError("Refusing to overwrite existing run: " + run_id)
    directory.mkdir(parents=True, exist_ok=True)
    config = copy.deepcopy(config)
    config["source_commit"] = subprocess.check_output(
        ["git", "rev-parse", "HEAD"], text=True).strip()
    atomic(directory / "config.json", config)
    status = dict(run_id=run_id, architecture=config["architecture"], seed=seed,
                  status="running", pid=os.getpid(), started_at=time.time())

    def state(**kwargs):
        status.update(kwargs, timestamp=time.time())
        atomic(directory / "status.json", status)

    state()
    model = make_model(config["architecture"], num_actions=config.get("num_actions", 3))
    model = model.to(device)
    optimizer = torch.optim.Adam(model.parameters(), lr=config.get("lr", .001))
    envconfig = dict(config.get("env", {}))
    env = make_task(config.get("task", "interval"), config["num_envs"], device,
                    100000 + seed, envconfig)
    parent_hash = None
    if parent:
        cp = torch.load(parent, map_location=device, weights_only=False)
        model.load_state_dict(cp["model"])
        optimizer.load_state_dict(cp["optimizer"])
        parent_hash = hashlib.sha256(Path(parent).read_bytes()).hexdigest()
        restore_rng(cp["rng"])
    env.reset()
    save_checkpoint(directory / "initial.pt", model, optimizer, config, env,
                    "initial", 0, parent_sha256=parent_hash)
    atomic(directory / "initial_validation.json", validation(model, device, config))
    best = -float("inf")
    stages = [("imitation", config.get("imitation_updates", 0)),
              ("ppo", config.get("ppo_updates", 0))]
    try:
        for stage, total in stages:
            if not total:
                continue
            if stage == "ppo":
                env.generator.manual_seed(200000 + seed)
                env.reset()
            for u in range(1, total + 1):
                start = time.perf_counter()
                if stage == "imitation":
                    tp = 1. if u <= total // 2 else .8
                    rollout, mask, infos = collect(model, env, teacher_probability=tp)
                    losses = imitation_update(model, optimizer, rollout, mask, config)
                else:
                    rollout, mask, infos = collect(model, env)
                    losses = ppo_update(model, optimizer, rollout, mask, config)
                elapsed = time.perf_counter() - start
                val = validation(model, device, config,
                                 config.get("validation_envs", 256))
                score = val["success"] + .1 * val["reward"]
                row = dict(update=u, stage=stage, validation=val, losses=losses,
                           elapsed=elapsed,
                           transitions=u * config["num_envs"] * env.horizon)
                with (directory / "metrics.jsonl").open("a") as f:
                    f.write(json.dumps(row, allow_nan=False) + "\n")
                save_checkpoint(directory / (stage + "_final.pt"), model, optimizer,
                                config, env, stage, u, parent_sha256=parent_hash,
                                validation=val)
                if score > best:
                    best = score
                    import shutil
                    shutil.copyfile(directory / (stage + "_final.pt"),
                                    directory / (stage + "_best.pt"))
                    atomic(directory / "selection.json",
                           dict(validation_seed=700001, update=u, score=score,
                                criterion="success + 0.1 * reward"))
                state(update=u, stage=stage, validation=val,
                      transitions=row["transitions"])
                print(json.dumps(dict(run_id=run_id, **row)), flush=True)
        state(status="complete", finished_at=time.time())
    except BaseException as exc:
        state(status="failed", error=str(exc))
        raise


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--config", required=True)
    p.add_argument("--artifacts", required=True)
    p.add_argument("--run-id", required=True)
    p.add_argument("--parent")
    p.add_argument("--device", default="cpu")
    a = p.parse_args()
    c = json.loads(Path(a.config).read_text())
    c["device"] = a.device
    train(c, a.artifacts, a.run_id, a.parent)


if __name__ == "__main__":
    main()
