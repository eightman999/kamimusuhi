"""PyGeNN FBA backend — UNVERIFIED on this box.

Implements the same Shiu et al. (2024) LIF + delayed alpha synapse model
via the pygenn API. Must be validated on the GPU host before any real
run. Raises BackendUnavailable when pygenn is not installed.
"""
from __future__ import annotations

import time

from .backend import BackendUnavailable, FbaBackend
from .torch_backend import PARAMS, _load_connectome

try:
    import pygenn  # noqa: F401
    _HAVE_GENN = True
except ImportError:
    pygenn = None
    _HAVE_GENN = False


class GennBackend(FbaBackend):
    """PyGeNN adapter (same interface as the other backends)."""

    name = "genn"

    def __init__(self, data_dir=None, synthetic=True, synthetic_neurons=2000,
                 connectivity=0.01, runs_dir=None):
        if not _HAVE_GENN:
            raise BackendUnavailable("pygenn not installed")
        self.data_dir = data_dir
        self.synthetic = synthetic
        self.synthetic_neurons = synthetic_neurons
        self.connectivity = connectivity
        self.runs_dir = runs_dir

    def initialize(self, phenotype, batch_size, seed, device,
                   replicate_seeds=None):
        # Model construction sketch (validate on GPU host):
        #   - LIF neuron custom model: dV/dt = (g - (V - vRest)) / tauMem,
        #     threshold vThr, reset vReset, refractory tRefrac.
        #   - Alpha synapse with 1.8 ms dendritic delay via a ring buffer
        #     or GeNN's built-in delay support.
        #   - Sparse connectivity initialised from the connectome matrix.
        raise BackendUnavailable("genn backend not verified on this host")

    def reset(self): raise BackendUnavailable("genn unavailable")
    def set_inputs(self, drive): raise BackendUnavailable("genn unavailable")
    def step(self, n_steps=1): raise BackendUnavailable("genn unavailable")
    def run(self, duration_ms): raise BackendUnavailable("genn unavailable")
    def get_state_summary(self): raise BackendUnavailable("genn unavailable")
    def get_population_activity(self, groups):
        raise BackendUnavailable("genn unavailable")
    def checkpoint(self): raise BackendUnavailable("genn unavailable")
    def restore(self, blob): raise BackendUnavailable("genn unavailable")

    def capabilities(self):
        return {"supports_gpu": True, "supports_batch": True,
                "is_reference": False, "max_batch_hint": None}


def genn_version() -> str | None:
    if not _HAVE_GENN:
        return None
    return getattr(pygenn, "__version__", None)
