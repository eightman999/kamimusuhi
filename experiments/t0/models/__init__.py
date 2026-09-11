from .gru import GRUCore
from .lstm import LSTMCore
from .mlp import MLPCore
from .ssm import LeakyCore, TinySSMCore

ARCHITECTURES = ("mlp", "gru64", "gru128", "lstm64", "lstm128", "ssm", "leaky")


def make_model(architecture, obs_dim=8, num_actions=3):
    if architecture == "mlp":
        return MLPCore(obs_dim, num_actions)
    if architecture == "gru64":
        return GRUCore(obs_dim, num_actions, 64)
    if architecture == "gru128":
        return GRUCore(obs_dim, num_actions, 128)
    if architecture == "lstm64":
        return LSTMCore(obs_dim, num_actions, 64)
    if architecture == "lstm128":
        return LSTMCore(obs_dim, num_actions, 128)
    if architecture == "ssm":
        return TinySSMCore(obs_dim, num_actions, 64)
    if architecture == "leaky":
        return LeakyCore(obs_dim, num_actions, 32)
    raise ValueError(f"Unknown architecture: {architecture}")
