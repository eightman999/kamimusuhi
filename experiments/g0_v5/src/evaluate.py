"""Frozen label-free distance metrics, with oracle confined to this module.

The encoder receives observations only. We never fit probes, tune a metric,
choose an architecture, or normalize using oracle values. Last-four-step means
are predeclared matching summaries; mid-context cosine includes the immediate
switch and a four-step recovery summary. Synthetic truth is used only to label
fixed pairs and to audit shortcut behavior. All comparison thresholds are fixed
before pilot execution. Numerical epsilon=1e-12.
"""
from pathlib import Path
import time
import numpy as np

EPS = 1e-12


def raw_control(observations):
    return np.asarray(observations)


def _cosine(a, b):
    denom = np.linalg.norm(a, axis=-1) * np.linalg.norm(b, axis=-1)
    return np.sum(a*b, axis=-1) / np.maximum(denom, EPS)


def auc_score(labels, scores):
    """Mann-Whitney AUC with exact half-credit for ties, no sklearn needed."""
    y, s = np.asarray(labels), np.asarray(scores)
    if not np.isfinite(s).all():
        raise ValueError('nonfinite scores')
    pos, neg = s[y == 1], s[y == 0]
    if not len(pos) or not len(neg):
        raise ValueError('AUC requires both classes')
    ordered = np.sort(neg)
    low = np.searchsorted(ordered, pos, side='left')
    high = np.searchsorted(ordered, pos, side='right')
    return float(np.mean((low + high) / (2 * len(neg))))


def collapse_stats(array):
    z = np.asarray(array, dtype=np.float64)
    x = z.reshape(-1, z.shape[-1])
    if len(x) < 2 or not np.isfinite(x).all():
        raise ValueError('collapse statistics require finite multiple samples')
    # Deterministic systematic sample bounds covariance and cosine work.
    x = x[np.linspace(0, len(x)-1, min(8192, len(x)), dtype=int)]
    mean, std = x.mean(axis=0), x.std(axis=0)
    centered = x - mean
    covariance = centered.T @ centered / max(1, len(x)-1)
    spectrum = np.maximum(np.linalg.eigvalsh(covariance), 0)[::-1]
    total = float(spectrum.sum())
    probabilities = spectrum / max(total, EPS)
    positive = probabilities[probabilities > 0]
    rank = float(np.exp(-np.sum(positive*np.log(positive)))) if total > EPS else 0.0
    # Absolute full-collapse and per-coordinate thresholds
    # are reported separately. Extremely low effective rank (<10% of dimension
    # or <2) is a declared dimensional-collapse guard, including redundant
    # coordinates with nonzero individual variances.
    full = bool(std.max() < 1e-4)
    dead = std < 1e-4
    low_rank = bool(rank < max(2.0, .1 * x.shape[1]))
    dimensional = bool(not full and (dead.mean() >= .8 or low_rank))
    n = min(len(x), 1024)
    cos = _cosine(x[:n], x[np.linspace(len(x)-1, 0, n, dtype=int)])
    return {'mean': mean.tolist(), 'std': float(np.sqrt(np.mean(std**2))),
            'per_dim_std': std.tolist(), 'effective_rank': rank,
            'covariance_spectrum': spectrum.tolist(),
            'cosine_distribution': {'mean': float(cos.mean()), 'std': float(cos.std()),
                'quantiles': np.quantile(cos, [0,.05,.25,.5,.75,.95,1]).tolist()},
            'full_collapse': full, 'dimensional_collapse': dimensional,
            'dead_dimension_fraction': float(dead.mean()), 'low_effective_rank': low_rank,
            'shortcut_context': False}


def _shortcut_diagnostics(z, cause, context):
    """Oracle-only variance explanation; no learned classifier or model fit.

    Context-only is flagged conservatively when >=95% of latent variation is
    between contexts AND pair matching is at chance
    (the caller adds the last condition). Context and cause diagnostics are
    descriptive: compositional sampling may correlate the two oracle factors.
    """
    x = np.asarray(z, dtype=np.float64)
    def explained(labels):
        mu = x.mean(axis=0)
        total = np.sum((x-mu)**2)
        between = sum(np.count_nonzero(labels == k) *
                      np.sum((x[labels == k].mean(axis=0)-mu)**2)
                      for k in np.unique(labels))
        return float(between / max(float(total), EPS))
    return {'context_variance_fraction': explained(context),
            'cause_variance_fraction': explained(cause)}


def evaluate_representation(encode, data_dir: Path, output_dir: Path | None = None):
    start = time.monotonic()
    data_dir = Path(data_dir)
    evaluation = data_dir / 'evaluation'
    predictions, stats_arrays = {}, {}
    result = {k: {} for k in ['iid', 'midctx', 'match_ood', 'dynseg_ood',
                              'combo_oodctx', 'intervention', 'collapse', 'runtime']}
    result['failure_class'] = 'NONE'

    def encoded(ds):
        obs = ds['obs']
        z = np.asarray(encode(obs.copy()))
        if z.ndim != 3 or z.shape[:2] != obs.shape[:2] or z.shape[-1] < 1:
            raise ValueError('encoder must return [N,T,Z]')
        if not np.isfinite(z).all():
            raise FloatingPointError('nonfinite latent')
        return z.astype(np.float64)

    all_shortcuts = []
    for split, key in [('iid_test', 'iid'), ('match_ood', 'match_ood'),
                       ('dynseg_ood', 'dynseg_ood'), ('combo_oodctx', 'combo_oodctx')]:
        with np.load(evaluation / (split + '.npz'), allow_pickle=False) as ds:
            z = encoded(ds)
            pooled = z[:, -4:].mean(axis=1)
            indices, labels = ds['pair_index'], ds['pair_label']
            distances = np.linalg.norm(pooled[indices[:,0]] - pooled[indices[:,1]], axis=1)
            auc = auc_score(labels, -distances)
            context = ds['context'][:, -1]
            cause = ds['cause'][:, -1]
            diag = _shortcut_diagnostics(pooled, cause, context)
            shortcut = bool(diag['context_variance_fraction'] >= .95 and
                            auc <= .55)
            all_shortcuts.append(shortcut)
            result[key] = {'auc': auc, 'n_pairs': int(len(labels)),
                'same_distance': float(distances[labels == 1].mean()),
                'different_distance': float(distances[labels == 0].mean()),
                'shortcut_diagnostics': diag}
            for name, values in [('score', -distances), ('label', labels),
                                  ('pooled', pooled), ('cause', cause), ('context', context)]:
                predictions[f'{key}_{name}'] = values
            if key == 'iid':
                result['collapse'] = collapse_stats(z)
                stats_arrays.update(latent_mean=np.array(result['collapse']['mean']),
                    per_dim_std=np.array(result['collapse']['per_dim_std']),
                    covariance_spectrum=np.array(result['collapse']['covariance_spectrum']),
                    effective_rank=np.array(result['collapse']['effective_rank']),
                    cosine_quantiles=np.array(result['collapse']['cosine_distribution']['quantiles']),
                    latent_std=np.array(result['collapse']['std']))

    with np.load(evaluation / 'midctx.npz', allow_pickle=False) as ds:
        z = encoded(ds)
        switch = int(ds['switch_time'])
        immediate = _cosine(z[:, switch-1], z[:, switch])
        recovery = _cosine(z[:, switch-4:switch].mean(axis=1),
                           z[:, switch:switch+4].mean(axis=1))
        result['midctx'] = {'stability': float(immediate.mean()),
            'recovery_stability': float(recovery.mean()), 'n': int(len(z))}
        predictions['midctx_stability'] = immediate
        predictions['midctx_recovery_stability'] = recovery

    with np.load(evaluation / 'intervention.npz', allow_pickle=False) as ds:
        z = encoded(ds).mean(axis=1)
        indices = ds['intervention_index']
        distances = np.linalg.norm(z[indices[:,1:]] - z[indices[:,0,None]], axis=-1)
        dc, dk, dn = distances.mean(axis=0)
        result['intervention'] = {'selectivity': float(dc/(dk+dn+EPS)),
            'cause_distance': float(dc), 'context_distance': float(dk),
            'nuisance_distance': float(dn), 'n': int(len(indices))}
        predictions['intervention_distances'] = distances

    collapse = result['collapse']
    # Require context-only behavior on both IID and explicitly cross-context
    # matching. The other split diagnostics remain independently inspectable.
    collapse['shortcut_context'] = bool(all_shortcuts[0] and all_shortcuts[1])
    if collapse['full_collapse']:
        result['failure_class'] = 'COLLAPSE_FULL'
    elif collapse['dimensional_collapse']:
        result['failure_class'] = 'COLLAPSE_DIM'
    elif collapse['shortcut_context']:
        result['failure_class'] = 'SHORTCUT_CONTEXT'
    result['runtime'] = {'evaluation_seconds': time.monotonic()-start}
    if output_dir is not None:
        output_dir = Path(output_dir)
        output_dir.mkdir(parents=True, exist_ok=True)
        for name in ['eval_predictions.npz', 'latent_stats.npz']:
            if (output_dir / name).exists():
                raise FileExistsError(f'refusing to overwrite {name}')
        np.savez_compressed(output_dir / 'eval_predictions.npz', **predictions)
        np.savez_compressed(output_dir / 'latent_stats.npz', **stats_arrays)
    return result
