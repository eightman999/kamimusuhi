"""PCA trajectories of hidden state over the delay period."""
import torch


def pca(states, components=3):
    """``states``: [M, H] -> (coords [M, K], explained_variance_ratio [K])."""
    centered = states - states.mean(0)
    _, s, v = torch.linalg.svd(centered, full_matrices=False)
    k = min(components, v.shape[0])
    coords = centered @ v[:k].T
    explained = s[:k].square() / (centered.shape[0] - 1)
    total = s.square().sum() / (centered.shape[0] - 1)
    return coords, (explained / total.clamp_min(1e-12))


def per_delay_trajectories(states, delays, max_len=None):
    """Mean PC trajectory per delay value.

    ``states``: [T, N, H]; ``delays``: [N].  Returns {delay: [T, K] coords}
    plus the explained-variance ratio of the fitted PCA.
    """
    t_len, n, h = states.shape
    flat = states.reshape(-1, h)
    coords, explained = pca(flat, components=3)
    coords = coords.reshape(t_len, n, -1)
    out = {}
    for d in delays.unique().tolist():
        sel = coords[:, delays == d].mean(1)
        if max_len:
            sel = sel[:max_len]
        out[int(d)] = sel.cpu().tolist()
    return {"explained": explained.cpu().tolist(), "trajectories": out}
