"""FBA backend interface.

A backend simulates a Phenotype (FBA0 base + artificial organs) for a
population batch of individuals and reports summary statistics. Full
spike trains are never returned through this interface — only summaries.
"""
from __future__ import annotations

from abc import ABC, abstractmethod

# {"rates_hz": {neuron_or_group: float}, "silence": [ids]}
InputDrive = dict
StepStats = dict        # simulated_ms, wall_s, spikes_total
StateSummary = dict     # t_ms, mean_rate_hz, active_fraction, per-batch counts, vram_bytes|None
BackendCapabilities = dict  # supports_gpu, supports_batch, is_reference, max_batch_hint


def validate_neuron_selection(neuron_indices, lane, n_neurons, batch_size):
    """Bound observation work without sampling from a simulator RNG."""
    if (type(lane) is not int or not 0 <= lane < batch_size
            or not isinstance(neuron_indices, list)
            or not 1 <= len(neuron_indices) <= 512
            or any(type(i) is not int or not 0 <= i < n_neurons
                   for i in neuron_indices)
            or len(set(neuron_indices)) != len(neuron_indices)):
        raise ValueError("invalid neuron activity selection")


class BackendUnavailable(RuntimeError):
    """Raised when a backend's dependencies are not installed."""


class FbaBackend(ABC):
    name: str = "abstract"

    @abstractmethod
    def initialize(self, phenotype: dict, batch_size: int, seed: int,
                   device: str, replicate_seeds: list[int] | None = None,
                   timer=None) -> None:
        """Build the network for ``batch_size`` lanes. Lane ``b`` draws
        its stochastic drive from a stream seeded by ``replicate_seeds[b]``
        (default ``replicate_seed(seed, b)``); the network itself is
        built from ``seed``. See fba/replicates.py."""
        ...

    @abstractmethod
    def reset(self) -> None:
        ...

    @abstractmethod
    def set_inputs(self, drive: InputDrive) -> None:
        ...

    @abstractmethod
    def step(self, n_steps: int = 1) -> None:
        ...

    @abstractmethod
    def run(self, duration_ms: float) -> StepStats:
        ...

    @abstractmethod
    def get_state_summary(self) -> StateSummary:
        ...

    @abstractmethod
    def get_population_activity(self, groups: list[str]) -> dict[str, list[float]]:
        ...

    def get_neuron_activity(self, neuron_indices: list[int], lane: int = 0):
        """Copy bounded cumulative counts from one lane, without advancing
        or resetting the simulator. Unsupported backends return None.
        This is window telemetry, not a full spike train or a metric input.
        """
        return None

    @abstractmethod
    def checkpoint(self) -> bytes:
        ...

    @abstractmethod
    def restore(self, blob: bytes) -> None:
        ...

    @abstractmethod
    def capabilities(self) -> BackendCapabilities:
        ...

    def semantics(self) -> dict:
        """Simulator identity of *this backend* (see fba/semantics.py):
        which equations, which RNG protocol, which propagation path.
        Recorded on every evaluation and checked by replay. Backends that
        have never been revised keep version 1; the version numbers are
        scoped to the backend family, not global."""
        from .semantics import unversioned_semantics

        return unversioned_semantics(self.name)

    def dataset_identity(self) -> dict:
        """Logical identity of the connectome data this backend simulates
        (``dataset_id``, ``version``, ``manifest_hash``, ``region_mode``).
        Recorded on every evaluation; never a raw filesystem path."""
        return {"dataset_id": self.name, "version": None,
                "manifest_hash": None, "region_mode": None}
