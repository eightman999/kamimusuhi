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


class BackendUnavailable(RuntimeError):
    """Raised when a backend's dependencies are not installed."""


class FbaBackend(ABC):
    name: str = "abstract"

    @abstractmethod
    def initialize(self, phenotype: dict, batch_size: int, seed: int,
                   device: str) -> None:
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

    @abstractmethod
    def checkpoint(self) -> bytes:
        ...

    @abstractmethod
    def restore(self, blob: bytes) -> None:
        ...

    @abstractmethod
    def capabilities(self) -> BackendCapabilities:
        ...

    def dataset_identity(self) -> dict:
        """Logical identity of the connectome data this backend simulates
        (``dataset_id``, ``version``, ``manifest_hash``, ``region_mode``).
        Recorded on every evaluation; never a raw filesystem path."""
        return {"dataset_id": self.name, "version": None,
                "manifest_hash": None, "region_mode": None}
