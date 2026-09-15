"""Self-supervised objectives: exclusively observations and temporal positions."""
import torch
from torch.nn import functional as F


def variance_covariance(z):
    flat = z.reshape(-1, z.shape[-1])
    centered = flat - flat.mean(0)
    std = (centered.square().mean(0) + 1e-4).sqrt()
    variance = F.relu(1 - std).mean()
    covariance = centered.T @ centered / max(1, len(flat) - 1)
    off_diagonal = covariance - torch.diag_embed(covariance.diag())
    return variance, off_diagonal.square().sum() / flat.shape[-1]


def objective(model, observations, config):
    latent = model.encoder(observations)
    target = model.target_encoder(observations).detach() if model.target_encoder is not None else latent
    losses = []
    for predictor, horizon in zip(model.predictors, model.horizons):
        predicted = predictor(latent[:, :-horizon])
        future = target[:, horizon:]
        if model.method == 'gru':
            loss = F.mse_loss(predicted, observations[:, horizon:])
        elif model.method == 'cpc':
            # Each temporal index forms a batch classification problem. The
            # diagonal is the same trajectory; all other trajectories are negatives.
            p = F.normalize(predicted, dim=-1).transpose(0, 1)
            f = F.normalize(future, dim=-1).transpose(0, 1)
            logits = torch.bmm(p, f.transpose(1, 2)) / config['temperature']
            labels = torch.arange(len(observations), device=observations.device).expand(logits.shape[0], -1)
            loss = F.cross_entropy(logits.reshape(-1, logits.shape[-1]), labels.reshape(-1))
        else:
            invariance = F.mse_loss(predicted, future)
            # Regularize online representations, never solely the predictor;
            # JEPA's EMA targets receive no gradient.
            var_a, cov_a = variance_covariance(latent[:, :-horizon])
            var_b, cov_b = variance_covariance(latent[:, horizon:])
            loss = (config['invariance_weight'] * invariance
                    + config['variance_weight'] * (var_a + var_b) / 2
                    + config['covariance_weight'] * (cov_a + cov_b) / 2)
        losses.append(loss)
    return torch.stack(losses).mean(), latent
