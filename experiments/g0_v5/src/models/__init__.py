"""Observation-only temporal representation architectures."""
from copy import deepcopy

import torch
from torch import nn


class Encoder(nn.Module):
    def __init__(self, obs_dim=24, hidden_dim=64, latent_dim=32):
        super().__init__()
        self.gru = nn.GRU(obs_dim, hidden_dim, batch_first=True)
        self.projection = nn.Linear(hidden_dim, latent_dim)

    def forward(self, observations):
        return self.projection(self.gru(observations)[0])


class TemporalModel(nn.Module):
    def __init__(self, config):
        super().__init__()
        self.method = config['method']
        self.horizons = tuple(config['horizons'])
        z, h = config['latent_dim'], config['hidden_dim']
        self.encoder = Encoder(config['obs_dim'], h, z)
        output_dim = config['obs_dim'] if self.method == 'gru' else z
        self.predictors = nn.ModuleList([
            nn.Sequential(nn.Linear(z, h), nn.GELU(), nn.Linear(h, output_dim))
            for _ in self.horizons
        ])
        self.target_encoder = deepcopy(self.encoder) if self.method == 'jepa' else None
        if self.target_encoder is not None:
            self.target_encoder.requires_grad_(False)

    @torch.no_grad()
    def update_target(self, decay):
        if self.target_encoder is not None:
            for target, online in zip(self.target_encoder.parameters(), self.encoder.parameters()):
                target.mul_(decay).add_(online, alpha=1 - decay)


def build_model(config):
    if config['method'] not in {'gru', 'cpc', 'vicreg', 'jepa'}:
        raise ValueError('unsupported method')
    return TemporalModel(config)
