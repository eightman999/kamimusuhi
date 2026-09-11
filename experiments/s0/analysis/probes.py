"""Linear probes on model latents (S0-H2).

From hidden activations only, can we read out:
  - which action was taken        (multiclass logistic probe)
  - the self-caused obs component (ridge probe, per-dim R2)
  - the world-caused component    (ridge probe, per-dim R2)
  - whether a disturbance hit     (binary probe)

Probe labels come from env ground truth and are used at eval time only.
"""

from __future__ import annotations

import numpy as np
import torch

from env.dynamics import CauseLabels


@torch.no_grad()
def collect_latents(model, obs: torch.Tensor, act: torch.Tensor):
    """Returns (latents (E*T,H), preds (E*T,D))."""
    model.eval()
    pred, h, _ = model(obs, act)
    e, t, d = pred.shape
    return h.reshape(e * t, -1), pred.reshape(e * t, d)


def _ridge_fit(X: np.ndarray, Y: np.ndarray, lam: float = 1e-3):
    Xb = np.concatenate([X, np.ones((X.shape[0], 1))], axis=1)
    A = Xb.T @ Xb + lam * np.eye(Xb.shape[1])
    W = np.linalg.solve(A, Xb.T @ Y)
    return W


def _ridge_r2(X, Y, W) -> np.ndarray:
    Xb = np.concatenate([X, np.ones((X.shape[0], 1))], axis=1)
    pred = Xb @ W
    ss_res = ((Y - pred) ** 2).sum(axis=0)
    ss_tot = ((Y - Y.mean(axis=0)) ** 2).sum(axis=0) + 1e-12
    return 1.0 - ss_res / ss_tot


def probe_components(latents: np.ndarray, action_comp: np.ndarray,
                     world_comp: np.ndarray, cause_labels: np.ndarray,
                     val_frac: float = 0.3, seed: int = 0) -> dict:
    """Ridge probes latent -> self/world obs components; R2 per dim,
    aggregated by cause group."""
    rng = np.random.default_rng(seed)
    n = latents.shape[0]
    idx = rng.permutation(n)
    n_val = int(n * val_frac)
    tr, va = idx[n_val:], idx[:n_val]

    w_self = _ridge_fit(latents[tr], action_comp[tr])
    r2_self = _ridge_r2(latents[va], action_comp[va], w_self)
    w_world = _ridge_fit(latents[tr], world_comp[tr])
    r2_world = _ridge_r2(latents[va], world_comp[va], w_world)

    def group_mean(r2, code):
        m = cause_labels == code
        return float(np.mean(r2[m])) if m.any() else float("nan")

    return {
        "r2_self_by_dim": r2_self.tolist(),
        "r2_world_by_dim": r2_world.tolist(),
        "r2_self_on_self_dims": group_mean(r2_self, CauseLabels.SELF),
        "r2_self_on_mix_dims": group_mean(r2_self, CauseLabels.MIXED),
        "r2_self_on_ext_dims": group_mean(r2_self, CauseLabels.EXTERNAL),
        "r2_world_on_ext_dims": group_mean(r2_world, CauseLabels.EXTERNAL),
    }


def probe_action(latents: np.ndarray, actions: np.ndarray,
                 n_actions: int = 4, steps: int = 2000,
                 val_frac: float = 0.3, seed: int = 0) -> dict:
    """Multinomial logistic probe latent -> action id."""
    rng = np.random.default_rng(seed)
    n = latents.shape[0]
    idx = rng.permutation(n)
    n_val = int(n * val_frac)
    tr, va = idx[n_val:], idx[:n_val]

    X = torch.as_tensor(latents, dtype=torch.float32)
    Y = torch.as_tensor(actions, dtype=torch.long)
    clf = torch.nn.Linear(X.shape[1], n_actions)
    opt = torch.optim.Adam(clf.parameters(), lr=1e-2)
    loss_fn = torch.nn.CrossEntropyLoss()
    for _ in range(steps):
        opt.zero_grad()
        loss = loss_fn(clf(X[tr]), Y[tr])
        loss.backward()
        opt.step()
    with torch.no_grad():
        acc = (clf(X[va]).argmax(1) == Y[va]).float().mean().item()
    return {"action_probe_acc": acc, "chance": 1.0 / n_actions}


def probe_disturbance(latents: np.ndarray, dist_flags: np.ndarray,
                      steps: int = 1500, val_frac: float = 0.3,
                      seed: int = 0) -> dict:
    """Binary probe: was this step's observation hit by an external
    disturbance impulse?"""
    if dist_flags.sum() == 0:
        return {"disturbance_probe_auc": float("nan")}
    rng = np.random.default_rng(seed)
    n = latents.shape[0]
    idx = rng.permutation(n)
    n_val = int(n * val_frac)
    tr, va = idx[n_val:], idx[:n_val]

    X = torch.as_tensor(latents, dtype=torch.float32)
    Y = torch.as_tensor(dist_flags.astype(np.float32))
    clf = torch.nn.Linear(X.shape[1], 1)
    opt = torch.optim.Adam(clf.parameters(), lr=1e-2)
    pos_w = torch.tensor([(1 - Y[tr].mean()) / (Y[tr].mean() + 1e-6)])
    loss_fn = torch.nn.BCEWithLogitsLoss(pos_weight=pos_w)
    for _ in range(steps):
        opt.zero_grad()
        loss = loss_fn(clf(X[tr]).squeeze(1), Y[tr])
        loss.backward()
        opt.step()
    with torch.no_grad():
        scores = clf(X[va]).squeeze(1).numpy()
    from .attribution import auc_score
    return {"disturbance_probe_auc": auc_score(Y[va].numpy() > 0.5, scores)}


def run_probes(model, ds: dict, device: str = "cpu", seed: int = 0) -> dict:
    obs = torch.as_tensor(ds["obs"], dtype=torch.float32, device=device)
    act = torch.as_tensor(ds["actions"], dtype=torch.long, device=device)
    lat, _ = collect_latents(model, obs, act)
    lat = lat.cpu().numpy()
    e, t = ds["actions"].shape
    flat = lambda x: x.reshape(e * t, *x.shape[2:]) if x.ndim > 2 else x.reshape(-1)
    out = {"n_latent": int(lat.shape[0]), "latent_dim": int(lat.shape[1])}
    out.update(probe_components(lat, flat(ds["action_comp"]),
                                flat(ds["world"]), ds["cause_labels"], seed=seed))
    out.update(probe_action(lat, flat(ds["actions"]), seed=seed))
    out.update(probe_disturbance(lat, flat(ds["disturbance"]), seed=seed))
    return out
