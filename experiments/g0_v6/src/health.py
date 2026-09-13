"""Label-free representation health and collapse diagnostics."""

import numpy as np
import torch


def _cosine(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    denom = np.linalg.norm(a, axis=-1) * np.linalg.norm(b, axis=-1)
    return np.sum(a * b, axis=-1) / np.maximum(denom, 1e-12)


def latent_statistics(latent, std_threshold=1e-4, active_fraction=0.2):
    """Return all declared geometry diagnostics without an auto low-rank stop."""
    z = np.asarray(latent, dtype=np.float64)
    if z.ndim < 2:
        raise ValueError("latent must have a feature dimension")
    x = z.reshape(-1, z.shape[-1])
    if len(x) < 2 or not np.isfinite(x).all():
        raise FloatingPointError("nonfinite representation")
    x = x[np.linspace(0, len(x) - 1, min(8192, len(x)), dtype=int)]
    mean = x.mean(axis=0)
    per_dim_std = x.std(axis=0)
    centered = x - mean
    covariance = centered.T @ centered / max(1, len(x) - 1)
    spectrum = np.maximum(np.linalg.eigvalsh(covariance), 0.0)[::-1]
    total = float(spectrum.sum())
    probabilities = spectrum / max(total, 1e-12)
    positive = probabilities[probabilities > 0]
    effective_rank = float(
        np.exp(-np.sum(positive * np.log(positive))) if total > 1e-12 else 0.0
    )
    rng = np.random.default_rng(817)
    sample_n = min(4096, max(2, len(x) * 2))
    first = rng.integers(len(x), size=sample_n)
    second = rng.integers(len(x), size=sample_n)
    second = np.where(first == second, (second + 1) % len(x), second)
    cosine = _cosine(x[first], x[second])
    hist, edges = np.histogram(cosine, bins=np.linspace(-1.0, 1.0, 41))
    dead_fraction = float((per_dim_std < std_threshold).mean())
    full = bool(float(per_dim_std.max()) < std_threshold)
    rank_threshold = max(2.0, 0.1 * x.shape[-1])
    dimensional = bool(not full and (dead_fraction >= 1.0 - active_fraction or effective_rank < rank_threshold))
    norms = np.linalg.norm(x, axis=-1)
    return {
        "latent_mean": mean.tolist(),
        "latent_std": float(np.sqrt(np.mean(per_dim_std**2))),
        "per_dimension_std": per_dim_std.tolist(),
        "latent_norm": {
            "mean": float(norms.mean()),
            "std": float(norms.std()),
            "quantiles": np.quantile(norms, [0, 0.05, 0.5, 0.95, 1]).tolist(),
        },
        "effective_rank": effective_rank,
        "effective_rank_ratio": float(effective_rank / max(1, x.shape[-1])),
        "effective_rank_threshold": float(rank_threshold),
        "dead_dimension_fraction": dead_fraction,
        "active_dimension_fraction": float((per_dim_std >= std_threshold).mean()),
        "covariance_spectrum": spectrum.tolist(),
        "pairwise_cosine_distribution": {
            "mean": float(cosine.mean()),
            "std": float(cosine.std()),
            "quantiles": np.quantile(cosine, [0, 0.05, 0.25, 0.5, 0.75, 0.95, 1]).tolist(),
            "histogram": hist.tolist(),
            "bin_edges": edges.tolist(),
        },
        "full_collapse": full,
        "dimensional_collapse_diagnostic": dimensional,
        "low_effective_rank_diagnostic": bool(effective_rank < rank_threshold),
        "failure_class": "COLLAPSE_FULL" if full else "NONE",
    }


@torch.no_grad()
def perturbation_sensitivity(model, observations, device, scale=0.01, batch_size=64):
    """Measure response to a fixed observation-only perturbation."""
    model.eval()
    rng = np.random.default_rng(1903)
    noise = rng.normal(0.0, scale, size=observations.shape).astype(np.float32)
    original, perturbed = [], []
    for start in range(0, len(observations), batch_size):
        x = torch.from_numpy(np.asarray(observations[start:start + batch_size], dtype=np.float32)).to(device)
        n = torch.from_numpy(noise[start:start + batch_size]).to(device)
        original.append(model.encoder(x).cpu().numpy())
        perturbed.append(model.encoder(x + n).cpu().numpy())
    a = np.concatenate(original)
    b = np.concatenate(perturbed)
    delta = b - a
    return {
        "noise_scale": float(scale),
        "mean_abs_delta": float(np.mean(np.abs(delta))),
        "rms_delta": float(np.sqrt(np.mean(delta**2))),
        "relative_rms_delta": float(np.sqrt(np.mean(delta**2)) / max(np.sqrt(np.mean(a**2)), 1e-12)),
        "cosine_after_perturbation": float(np.mean(_cosine(a.reshape(-1, a.shape[-1]), b.reshape(-1, b.shape[-1])))),
    }
