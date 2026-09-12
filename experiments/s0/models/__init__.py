from .mlp import MLPModel
from .gru import GRUModel

MODEL_REGISTRY = {
    "mlp_state": dict(cls=MLPModel, use_action=False, hidden=128),
    "mlp_state_action": dict(cls=MLPModel, use_action=True, hidden=128),
    "gru_state": dict(cls=GRUModel, use_action=False, hidden=64),
    "gru_state_action": dict(cls=GRUModel, use_action=True, hidden=64),
    "gru128_state_action": dict(cls=GRUModel, use_action=True, hidden=128),
}


def build_model(name: str, obs_dim: int, n_actions: int, target_delta: bool = True):
    spec = MODEL_REGISTRY[name]
    return spec["cls"](
        obs_dim=obs_dim,
        n_actions=n_actions,
        hidden=spec["hidden"],
        use_action=spec["use_action"],
        target_delta=target_delta,
    )
