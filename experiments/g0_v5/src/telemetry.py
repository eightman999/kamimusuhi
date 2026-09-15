"""Observation-derived representation health; no evaluation dependencies."""
import numpy as np


def latent_statistics(latent, std_threshold=1e-4, active_fraction=0.2):
    x = np.asarray(latent, dtype=np.float64).reshape(-1, latent.shape[-1])
    if not np.isfinite(x).all():
        raise FloatingPointError('nonfinite representation')
    mean, std = x.mean(0), x.std(0)
    cov = np.cov(x, rowvar=False)
    spectrum = np.maximum(np.linalg.eigvalsh(cov), 0)
    proportions = spectrum / max(spectrum.sum(), 1e-30)
    rank = float(np.exp(-(proportions * np.log(np.maximum(proportions, 1e-30))).sum())) if spectrum.sum() > 1e-20 else 0.0
    rng = np.random.default_rng(817)
    a, b = rng.integers(len(x), size=(2, min(4096, len(x) * 2)))
    b = np.where(a == b, (b + 1) % len(x), b)
    cosine = (x[a] * x[b]).sum(1) / np.maximum(np.linalg.norm(x[a], axis=1) * np.linalg.norm(x[b], axis=1), 1e-30)
    active = float((std >= std_threshold).mean())
    rank_threshold = max(2.0, 0.1 * x.shape[-1])
    dead_fraction = float((std < std_threshold).mean())
    failure = 'COLLAPSE_FULL' if std.max() < std_threshold else ('COLLAPSE_DIM' if dead_fraction >= 1.0 - active_fraction or rank < rank_threshold else 'NONE')
    hist, edges = np.histogram(cosine, bins=np.linspace(-1, 1, 41))
    return dict(latent_mean=mean.tolist(), latent_std=float(x.std()), per_dimension_std=std.tolist(),
                effective_rank=rank, effective_rank_threshold=rank_threshold, dead_dimension_fraction=dead_fraction, covariance_spectrum=spectrum.tolist(), active_dimension_fraction=active,
                pairwise_cosine=dict(mean=float(cosine.mean()), std=float(cosine.std()),
                                     quantiles=np.quantile(cosine, [0, .05, .25, .5, .75, .95, 1]).tolist(),
                                     histogram=hist.tolist(), bin_edges=edges.tolist()), failure_class=failure)
