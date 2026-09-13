"""Raw/PCA controls and the fixed label-free future-observation readout."""

import numpy as np


def raw_encode(observations):
    return np.asarray(observations)


class PCAControl:
    def __init__(self, training_obs, dimensions=16):
        x = np.asarray(training_obs, dtype=np.float64).reshape(-1, training_obs.shape[-1])
        self.mean = x.mean(axis=0)
        _, _, vh = np.linalg.svd(x - self.mean, full_matrices=False)
        self.components = vh[:dimensions]

    def __call__(self, observations):
        x = np.asarray(observations, dtype=np.float64)
        return ((x - self.mean) @ self.components.T).astype(np.float32)

    def save(self, path):
        np.savez(path, mean=self.mean, components=self.components)


def temporal_readout(encode, training_obs, validation_obs, horizon=4, alpha=0.01):
    """Fit a fixed train-only ridge map to future raw observations."""
    z = np.asarray(encode(training_obs), dtype=np.float64)[:, :-horizon:4]
    x = z.reshape(-1, z.shape[-1])
    y = np.asarray(training_obs[:, horizon::4], dtype=np.float64).reshape(-1, training_obs.shape[-1])
    mean = x.mean(axis=0)
    scale = np.maximum(x.std(axis=0), 1e-6)
    x = (x - mean) / scale
    target_mean = y.mean(axis=0)
    weights = np.linalg.solve(
        x.T @ x / len(x) + alpha * np.eye(x.shape[1]),
        x.T @ (y - target_mean) / len(x),
    )
    v = np.asarray(encode(validation_obs), dtype=np.float64)[:, :-horizon:4]
    validation_target = np.asarray(validation_obs[:, horizon::4], dtype=np.float64)
    prediction = (v - mean) / scale @ weights + target_mean
    return {
        "mse": float(np.mean((prediction - validation_target) ** 2)),
        "horizon": int(horizon),
        "alpha": float(alpha),
        "train_rows": int(len(x)),
        "validation_episodes": int(len(validation_obs)),
    }
