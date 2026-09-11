from .gru import GRUCore
from .lstm import LSTMCore
from .mlp import MLPCore
from .ssm import TinySSMCore

ARCHITECTURES = ("mlp", "gru64", "gru128", "lstm64", "ssm")


def make_model(arch, obs_dim=20, hidden=64):
    if arch == "mlp":
        return MLPCore(obs_dim, hidden_size=128)
    if arch == "gru64":
        return GRUCore(obs_dim, 64)
    if arch == "gru128":
        return GRUCore(obs_dim, 128)
    if arch == "lstm64":
        return LSTMCore(obs_dim, 64)
    if arch == "ssm":
        return TinySSMCore(obs_dim, hidden)
    raise ValueError(f"Unknown architecture: {arch}")
