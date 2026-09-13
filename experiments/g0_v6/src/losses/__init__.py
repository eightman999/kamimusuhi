"""Observation-only CPC InfoNCE objective."""

import torch
from torch.nn import functional as F


def info_nce_loss(model, observations: torch.Tensor, temperature: float):
    """Predict the same-trajectory future latent among temporal negatives."""
    latent = model.encoder(observations)
    losses = []
    for predictor, horizon in zip(model.predictors, model.horizons):
        predicted = predictor(latent[:, :-horizon])
        future = latent[:, horizon:]
        # At each temporal position, the diagonal trajectory is positive and
        # every other trajectory in the batch is a negative. No labels enter.
        p = F.normalize(predicted, dim=-1).transpose(0, 1)
        f = F.normalize(future, dim=-1).transpose(0, 1)
        logits = torch.bmm(p, f.transpose(1, 2)) / float(temperature)
        labels = torch.arange(len(observations), device=observations.device)
        labels = labels.expand(logits.shape[0], -1)
        losses.append(F.cross_entropy(logits.reshape(-1, logits.shape[-1]), labels.reshape(-1)))
    return torch.stack(losses).mean(), latent
