"""Observation-only analytic controls and heldout temporal readout.

No dependency on evaluation/oracle or old G0. PCA and ridge fit exclusively
on training observations, with fixed dimensions and regularization.
"""
from pathlib import Path
import numpy as np


def raw_encode(x):
    return np.asarray(x)


class PCAControl:
    def __init__(self, training_obs, dimensions=16):
        x = np.asarray(training_obs, dtype=np.float64).reshape(-1, training_obs.shape[-1])
        self.mean = x.mean(0)
        _, _, vh = np.linalg.svd(x - self.mean, full_matrices=False)
        self.components = vh[:dimensions]

    def __call__(self, x):
        return ((x - self.mean) @ self.components.T).astype(np.float32)

    def save(self, path):
        np.savez(path, mean=self.mean, components=self.components)


def temporal_readout(encode, training_obs, validation_obs, horizon=4, alpha=.01):
    """Fixed label-free linear future-observation probe, not a cause probe.

Uses disjoint episodes, stride 4 to bound compute, train-only scaling and
intercept. Comparison is in a common observation target space for all methods,
including EMA JEPA where native SSL loss is not a stationary target metric.
"""
    z = np.asarray(encode(training_obs), dtype=np.float64)[:, :-horizon:4]
    x = z.reshape(-1, z.shape[-1])
    y = training_obs[:, horizon::4].reshape(-1, training_obs.shape[-1]).astype(np.float64)
    mean, scale = x.mean(0), np.maximum(x.std(0), 1e-6)
    x = (x - mean) / scale
    ym = y.mean(0)
    w = np.linalg.solve(x.T @ x / len(x) + alpha * np.eye(x.shape[1]), x.T @ (y - ym) / len(x))
    v = np.asarray(encode(validation_obs), dtype=np.float64)[:, :-horizon:4]
    vy = validation_obs[:, horizon::4]
    prediction = (v - mean) / scale @ w + ym
    return {'mse': float(np.mean((prediction - vy) ** 2)), 'horizon': horizon,
            'alpha': alpha, 'train_rows': len(x), 'validation_episodes': len(validation_obs)}
