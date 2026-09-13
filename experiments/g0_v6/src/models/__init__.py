"""Fixed CPC encoder and per-horizon latent predictors."""

import torch
from torch import nn


class Encoder(nn.Module):
    def __init__(self, obs_dim: int, hidden_dim: int, latent_dim: int):
        super().__init__()
        self.gru = nn.GRU(obs_dim, hidden_dim, batch_first=True)
        self.projection = nn.Linear(hidden_dim, latent_dim)

    def forward(self, observations: torch.Tensor) -> torch.Tensor:
        return self.projection(self.gru(observations)[0])


class CPCModel(nn.Module):
    def __init__(self, config: dict):
        super().__init__()
        self.horizons = tuple(int(x) for x in config["horizons"])
        latent_dim = int(config["latent_dim"])
        hidden_dim = int(config["hidden_dim"])
        self.encoder = Encoder(int(config["obs_dim"]), hidden_dim, latent_dim)
        self.predictors = nn.ModuleList(
            nn.Sequential(
                nn.Linear(latent_dim, hidden_dim),
                nn.GELU(),
                nn.Linear(hidden_dim, latent_dim),
            )
            for _ in self.horizons
        )

    def predictor_for(self, horizon: int) -> nn.Module:
        try:
            return self.predictors[self.horizons.index(int(horizon))]
        except ValueError as exc:
            raise ValueError(f"horizon {horizon} is not a trained CPC horizon") from exc


def build_model(config: dict) -> CPCModel:
    if config.get("method") != "cpc":
        raise ValueError("G0-v6 fixes the method to cpc")
    return CPCModel(config)
