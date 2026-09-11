"""S0 evaluation battery: causal tests S-C1..S-C5, OOD, probes.

Usage:
    python evaluate.py --ckpt runs/gru_state_action__seed0/ckpt.pt
    python evaluate.py --run-dir runs/gru_state_action__seed0
"""

from __future__ import annotations

import argparse
import copy
import json
import sys
from pathlib import Path

import numpy as np
import torch

sys.path.insert(0, str(Path(__file__).parent))

from data import (EVAL_SEED_OFFSET, collect_dataset, load_config,  # noqa: E402
                  make_env_config)
from env import AgencyEnv  # noqa: E402
from env.dynamics import CauseLabels, N_ACTIONS, NOOP  # noqa: E402
from models import build_model  # noqa: E402
from analysis.attribution import (attribution_auc,  # noqa: E402
                                  last_action_sensitivity)
from analysis.probes import run_probes  # noqa: E402
from train import load_ckpt  # noqa: E402

GROUPS = {"self": CauseLabels.SELF, "external": CauseLabels.EXTERNAL,
          "mixed": CauseLabels.MIXED, "noise": CauseLabels.NOISE}


def per_group_mse(pred: torch.Tensor, target: torch.Tensor,
                  labels: np.ndarray) -> dict:
    err = ((pred - target) ** 2).mean(dim=(0, 1)).cpu().numpy()  # (D,)
    out = {"overall": float(err.mean())}
    for name, code in GROUPS.items():
        m = labels == code
        out[name] = float(err[m].mean()) if m.any() else float("nan")
    return out


def predict(model, ds: dict, device: str,
            actions: torch.Tensor | None = None) -> torch.Tensor:
    obs = torch.as_tensor(ds["obs"], dtype=torch.float32, device=device)
    act = torch.as_tensor(ds["actions"], dtype=torch.long, device=device)
    if actions is not None:
        act = actions.to(device)
    with torch.no_grad():
        pred, _, _ = model(obs, act)
    return pred


def target_tensor(ds: dict, device: str) -> torch.Tensor:
    return torch.as_tensor(ds["next_obs"], dtype=torch.float32, device=device)


def eval_sc1_shuffle(model, ds, labels, device, seed) -> dict:
    """S-C1: permute the action sequence within each episode."""
    rng = np.random.default_rng(seed)
    act = torch.as_tensor(ds["actions"], dtype=torch.long)
    sh = act.clone()
    for e in range(sh.shape[0]):
        sh[e] = sh[e][torch.as_tensor(rng.permutation(sh.shape[1]))]
    pred = predict(model, ds, device, sh)
    return per_group_mse(pred, target_tensor(ds, device), labels)


def eval_sc5_mask(model, ds, labels, device) -> dict:
    """S-C5: remove action information (all inputs = NOOP)."""
    act = torch.as_tensor(ds["actions"], dtype=torch.long)
    masked = torch.full_like(act, NOOP)
    pred = predict(model, ds, device, masked)
    return per_group_mse(pred, target_tensor(ds, device), labels)


def eval_sc2_counterfactual(model, env_cfg, env_seed, labels, device,
                            n_episodes=8, seed=0) -> dict:
    """S-C2: same state, alternative action. Compare the model's
    predicted difference to the env's true action-effect difference."""
    env = AgencyEnv(env_cfg, seed=env_seed, noise_seed=seed + 700_031)
    rng = np.random.default_rng(seed + 31_000)
    affected = np.isin(labels, [CauseLabels.SELF, CauseLabels.MIXED])

    diffs_true, diffs_pred = [], []
    for _ in range(n_episodes):
        obs = env.reset()
        h_obs, h_act = [obs.copy()], []
        for t in range(env_cfg.episode_len):
            a = int(rng.integers(N_ACTIONS))
            st = env.get_state()
            true_next = {}
            for a2 in range(N_ACTIONS):
                env.set_state(st)
                o2, _ = env.step(a2)
                true_next[a2] = o2
            env.set_state(st)
            obs, _ = env.step(a)
            h_obs.append(obs.copy())
            h_act.append(a)

            O = torch.as_tensor(np.stack(h_obs[:-1])[None],
                                dtype=torch.float32, device=device)
            with torch.no_grad():
                base_act = torch.as_tensor([h_act], dtype=torch.long,
                                           device=device)
                p_base, _, _ = model(O, base_act)
                p_base = p_base[0, -1].cpu().numpy()
                for a2 in range(N_ACTIONS):
                    if a2 == a:
                        continue
                    aa = base_act.clone()
                    aa[0, -1] = a2
                    p2, _, _ = model(O, aa)
                    diffs_pred.append(p2[0, -1].cpu().numpy() - p_base)
                    diffs_true.append(true_next[a2] - true_next[a])
    dt = np.stack(diffs_true)
    dp = np.stack(diffs_pred)
    dt_a, dp_a = dt[:, affected], dp[:, affected]
    mse = float(((dt_a - dp_a) ** 2).mean())
    num = (dt_a * dp_a).sum(axis=1)
    den = np.linalg.norm(dt_a, axis=1) * np.linalg.norm(dp_a, axis=1) + 1e-9
    cos = float(np.mean(num / den))
    return {"counterfactual_mse_affected": mse, "counterfactual_cos": cos,
            "n_pairs": len(diffs_true)}


def eval_ood(model, cfg, env_seed, seed, device) -> dict:
    """§10 OOD: unseen disturbance, action gain, coupling, delay."""
    labels = None
    out = {}
    o = cfg["eval"]["ood"]
    cases = {
        "disturbance": dict(disturbance_prob=cfg["env"]["disturbance_prob"]
                            * o["disturbance_prob_mul"],
                            disturbance_gain=cfg["env"]["disturbance_gain"]
                            * o["disturbance_gain_mul"]),
        "action_gain": dict(action_gain=cfg["env"]["action_gain"]
                            * o["action_gain_mul"]),
        "sensor_coupling": dict(sensor_coupling=o["sensor_coupling"]),
        "action_delay": dict(action_delay=o["action_delay"]),
    }
    for name, ov in cases.items():
        ec = make_env_config(cfg["env"], **ov)
        ds = collect_dataset(ec, cfg["eval"]["episodes"],
                             seed + EVAL_SEED_OFFSET + 9_000,
                             env_seed=env_seed)
        labels = ds["cause_labels"]
        pred = predict(model, ds, device)
        out[name] = per_group_mse(pred, target_tensor(ds, device), labels)
    return out


def _finetune(model, adapt: dict, steps: int, lr: float,
              device: str) -> None:
    opt = torch.optim.Adam(model.parameters(), lr=lr)
    obs = torch.as_tensor(adapt["obs"], dtype=torch.float32, device=device)
    nxt = torch.as_tensor(adapt["next_obs"], dtype=torch.float32,
                          device=device)
    act = torch.as_tensor(adapt["actions"], dtype=torch.long,
                          device=device)
    loss_fn = torch.nn.MSELoss()
    n = obs.shape[0]
    model.train()
    for _ in range(steps):
        idx = torch.randint(0, n, (min(32, n),), device=device)
        loss = loss_fn(model(obs[idx], act[idx])[0], nxt[idx])
        opt.zero_grad(); loss.backward(); opt.step()
    model.eval()


def eval_sc4_perm(model, cfg, env_seed, seed, device,
                  model_name=None, target_delta=True) -> dict:
    """S-C4: permute actuator effects; measure degradation, then
    readapt on a small budget (S0-H5). A scratch control trained on the
    same budget shows whether recovery uses transferred structure."""
    ec = make_env_config(cfg["env"])
    env = AgencyEnv(ec, seed=env_seed, noise_seed=seed + 710_037)
    perm = np.array([1, 2, 0, 3])  # MOVE_A<->MOVE_B etc., NOOP fixed
    env.set_action_permutation(perm)

    ds = collect_dataset(ec, cfg["eval"]["episodes"],
                         seed + EVAL_SEED_OFFSET + 5_000, env=env)
    labels = ds["cause_labels"]
    pred = predict(model, ds, device)
    before = per_group_mse(pred, target_tensor(ds, device), labels)

    # small-budget readaptation on the permuted actuator
    adapt = collect_dataset(ec, cfg["eval"]["adapt"]["episodes"],
                            seed + EVAL_SEED_OFFSET + 6_000, env=env)
    steps = cfg["eval"]["adapt"]["steps"]
    lr = cfg["eval"]["adapt"]["lr"]
    tgt = target_tensor(ds, device)

    m2 = copy.deepcopy(model).to(device)
    _finetune(m2, adapt, steps, lr, device)
    after = per_group_mse(predict(m2, ds, device), tgt, labels)

    # scratch control: fresh model, same data budget — isolates how much
    # of the recovery is transferred structure vs just "200 steps suffice"
    out = {"perm": perm.tolist(), "mse_before": before,
           "mse_after": after}
    if model_name is not None:
        torch.manual_seed(seed + 9_000)
        m3 = build_model(model_name, model.obs_dim, model.n_actions,
                         target_delta).to(device)
        _finetune(m3, adapt, steps, lr, device)
        out["mse_scratch"] = per_group_mse(predict(m3, ds, device),
                                           tgt, labels)
    return out


def eval_cross_world(model, cfg, env_seed, seed, device,
                     model_name=None, target_delta=True) -> dict:
    """Cross-world test: same EnvConfig but a different env_seed → new
    obs permutation, new sensor mixing (W), new action maps, new AR.
    Measures transfer + small-budget readaptation on the new world."""
    ec = make_env_config(cfg["env"])
    new_seed = env_seed + 999_331
    env = AgencyEnv(ec, seed=new_seed, noise_seed=seed + 720_043)

    ds = collect_dataset(ec, cfg["eval"]["episodes"],
                         seed + EVAL_SEED_OFFSET + 7_000, env=env)
    labels = ds["cause_labels"]
    tgt = target_tensor(ds, device)
    before = per_group_mse(predict(model, ds, device), tgt, labels)

    sens = last_action_sensitivity(
        model,
        torch.as_tensor(ds["obs"], dtype=torch.float32, device=device),
        torch.as_tensor(ds["actions"], dtype=torch.long, device=device),
        N_ACTIONS)
    attr = attribution_auc(sens, labels)

    adapt = collect_dataset(ec, cfg["eval"]["adapt"]["episodes"],
                            seed + EVAL_SEED_OFFSET + 8_000, env=env)
    m2 = copy.deepcopy(model).to(device)
    _finetune(m2, adapt, cfg["eval"]["adapt"]["steps"],
              cfg["eval"]["adapt"]["lr"], device)
    after = per_group_mse(predict(m2, ds, device), tgt, labels)

    out = {"new_env_seed": new_seed, "mse_before": before,
           "mse_after": after, "attr_auc_before": attr["auc_self_or_mix"]}
    if model_name is not None:
        torch.manual_seed(seed + 9_500)
        m3 = build_model(model_name, model.obs_dim, model.n_actions,
                         target_delta).to(device)
        _finetune(m3, adapt, cfg["eval"]["adapt"]["steps"],
                  cfg["eval"]["adapt"]["lr"], device)
        out["mse_scratch"] = per_group_mse(predict(m3, ds, device),
                                           tgt, labels)
    return out


def eval_battery(ckpt_path: str | Path, config_path: str | Path | None = None,
                 device: str = "cpu", probes: bool = True) -> dict:
    model, ck = load_ckpt(ckpt_path, device)
    cfg = ck["config"] if config_path is None else load_config(config_path)
    env_seed = ck["env_seed"]
    seed = ck["seed"]
    env_cfg = make_env_config(cfg["env"])

    ds = collect_dataset(env_cfg, cfg["eval"]["episodes"],
                         seed + EVAL_SEED_OFFSET, env_seed=env_seed)
    labels = ds["cause_labels"]

    base = per_group_mse(predict(model, ds, device),
                         target_tensor(ds, device), labels)
    out = {"model": ck["model_name"], "seed": seed, "env_seed": env_seed,
           "base_mse": base}

    out["sc1_shuffle"] = eval_sc1_shuffle(model, ds, labels, device, seed)
    out["sc5_mask"] = eval_sc5_mask(model, ds, labels, device)
    out["sc2_counterfactual"] = eval_sc2_counterfactual(
        model, env_cfg, env_seed, labels, device, seed=seed)
    out["sc4_permutation"] = eval_sc4_perm(
        model, cfg, env_seed, seed, device,
        model_name=ck["model_name"], target_delta=ck["target_delta"])
    out["ood"] = eval_ood(model, cfg, env_seed, seed, device)
    out["cross_world"] = eval_cross_world(
        model, cfg, env_seed, seed, device,
        model_name=ck["model_name"], target_delta=ck["target_delta"])

    sens = last_action_sensitivity(
        model,
        torch.as_tensor(ds["obs"], dtype=torch.float32, device=device),
        torch.as_tensor(ds["actions"], dtype=torch.long, device=device),
        N_ACTIONS)
    out["attribution"] = attribution_auc(sens, labels)

    if probes:
        out["probes"] = run_probes(model, ds, device, seed)
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--ckpt", default=None)
    ap.add_argument("--run-dir", default=None)
    ap.add_argument("--config", default=None)
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--no-probes", action="store_true")
    ap.add_argument("--out", default=None)
    args = ap.parse_args()

    ckpt = args.ckpt or str(Path(args.run_dir) / "ckpt.pt")
    res = eval_battery(ckpt, args.config, args.device,
                       probes=not args.no_probes)
    text = json.dumps(res, indent=2)
    print(text)
    out = Path(args.out) if args.out else Path(ckpt).parent / "eval.json"
    out.write_text(text)


if __name__ == "__main__":
    main()
