from .gru import GRUCore
from .mlp import MLPCore
from .sparse_rnn import SparseRNNCore
from .softlogic import SoftLogicCore
from .fly_modular import FlyModularCore


def make_model(architecture, hidden_size=128, density=.1):
    if architecture in ("gru64", "gru128"):
        return GRUCore(64 if architecture == "gru64" else 128)
    if architecture == "mlp":
        return MLPCore(hidden_size)
    if architecture == "sparse_rnn":
        return SparseRNNCore(hidden_size, density)
    if architecture == "softlogic_rnn":
        return SoftLogicCore(hidden_size)
    if architecture == "fly_modular":
        return FlyModularCore(hidden_size, density)
    raise ValueError(f"Unknown architecture: {architecture}")
