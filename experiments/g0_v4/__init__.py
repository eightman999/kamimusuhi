"""G0-v4 — Predictive Invariant Grounding.

Same latent-cause environment and evaluation battery as G0
(`experiments.g0`), new self-supervised objectives:

    gru     action-conditioned next-obs predictor (control, == G0 gru)
    cpc     contrastive predictive coding (InfoNCE on future latents)
    vicreg  temporal VICReg over augmented views + latent prediction
    jepa    latent prediction with EMA target encoder

Training uses obs/next_obs/actions ONLY. Cause/context/composition
labels remain eval-only metadata, enforced by protocol_validator.
"""
