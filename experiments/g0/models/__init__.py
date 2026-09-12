from .net import AEModel, AEVQModel, GRUPredModel, GRUVQModel
from .vq import VectorQuantizerEMA

# name -> (class, kwargs taken from ModelConfig)
MODEL_REGISTRY = {
    "ae": dict(cls=AEModel, hidden_key="ae_hidden", latent_key="ae_latent"),
    "ae_vq": dict(cls=AEVQModel, hidden_key="ae_hidden",
                  latent_key="ae_latent"),
    "gru": dict(cls=GRUPredModel, hidden_key="hidden"),
    "gru_vq": dict(cls=GRUVQModel, hidden_key="hidden"),
}

TRAINED_MODELS = list(MODEL_REGISTRY)


def build_model(name: str, obs_dim: int, n_actions: int, mcfg,
                target_delta: bool = True):
    spec = MODEL_REGISTRY[name]
    cls = spec["cls"]
    if name in ("ae", "ae_vq"):
        kw = dict(hidden=mcfg.ae_hidden, latent=mcfg.ae_latent)
        if name == "ae_vq":
            kw.update(n_codes=mcfg.codebook_size,
                      commitment=mcfg.commitment)
    else:
        kw = dict(n_actions=n_actions, hidden=mcfg.hidden,
                  target_delta=target_delta)
        if name == "gru_vq":
            kw.update(n_codes=mcfg.codebook_size,
                      commitment=mcfg.commitment)
    return cls(obs_dim=obs_dim, **kw)


def model_spec(name: str, mcfg, target_delta: bool) -> dict:
    """Serializable description stored in checkpoints."""
    return {"name": name, "hidden": mcfg.hidden, "ae_hidden": mcfg.ae_hidden,
            "ae_latent": mcfg.ae_latent, "codebook_size": mcfg.codebook_size,
            "commitment": mcfg.commitment, "target_delta": target_delta}
