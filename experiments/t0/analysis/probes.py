"""Linear probes from hidden state to elapsed time / remaining interval / phase.

Closed-form ridge regression; no external dependency.  Splits by episode, not
by timestep, so train and test rows never come from the same rollout.
"""
import torch


def ridge_fit(x, y, lam=1e-3):
    x = torch.cat([x, torch.ones(x.shape[0], 1, device=x.device)], dim=1)
    gram = x.T @ x + lam * torch.eye(x.shape[1], device=x.device)
    return torch.linalg.solve(gram, x.T @ y)


def r2_score(y, pred):
    residual = (y - pred).square().sum(0)
    total = (y - y.mean(0)).square().sum(0).clamp_min(1e-8)
    return 1. - residual / total


def probe_targets(t, target_step, horizon):
    """Regression targets for step ``t`` in an episode with deadline T*."""
    elapsed = float(t)
    remaining = float(target_step - t)
    phase = float(t) / max(float(target_step), 1.)
    return elapsed, remaining, phase


def fit_time_probes(states, target_steps, mask=None, lam=1e-3, test_frac=.25,
                    seed=0):
    """``states``: [T, N, H] hidden states; ``target_steps``: [N].

    ``mask``: optional [T, N] bool restricting rows to alive in-episode steps.
    Episodes are split into train/test so no rollout appears in both.

    Returns R^2 for {elapsed, remaining, phase} plus a shuffle control.
    """
    t_len, n, _ = states.shape
    steps = torch.arange(t_len, device=states.device).unsqueeze(1).expand(t_len, n)
    targets = torch.stack([
        steps.float(),
        target_steps.unsqueeze(0).float() - steps.float(),
        steps.float() / target_steps.clamp_min(1).unsqueeze(0).float()],
        dim=-1)
    x = states.reshape(-1, states.shape[-1])
    y = targets.reshape(-1, 3)
    env_index = torch.arange(n, device=states.device).expand(t_len, n).reshape(-1)
    if mask is not None:
        keep = mask.reshape(-1).bool()
        x, y, env_index = x[keep], y[keep], env_index[keep]
    g = torch.Generator(device="cpu").manual_seed(seed)
    order = torch.randperm(n, generator=g)
    n_test = max(1, int(n * test_frac))
    test_set = set(order[:n_test].tolist())
    row_is_test = torch.tensor([int(e) in test_set for e in env_index.tolist()],
                               device=x.device)
    w = ridge_fit(x[~row_is_test], y[~row_is_test], lam)
    pred = torch.cat([x[row_is_test],
                      torch.ones(int(row_is_test.sum()), 1, device=x.device)], 1) @ w
    r2 = r2_score(y[row_is_test], pred)
    shuffled = y[row_is_test][torch.randperm(int(row_is_test.sum()), generator=g)]
    shuffle_r2 = r2_score(shuffled, pred)
    return {"elapsed_r2": float(r2[0]), "remaining_r2": float(r2[1]),
            "phase_r2": float(r2[2]), "shuffle_max_r2": float(shuffle_r2.max()),
            "n_train_rows": int((~row_is_test).sum()),
            "n_test_rows": int(row_is_test.sum())}
