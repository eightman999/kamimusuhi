"""CX0 organ signal contract — the fixed input every architecture arm sees.

`OrganSignals` is the Python mirror of the PR #39 `OrganSignal` concept:
attributed, transient, derived — never canonical state. Field order and dims
are fixed so that shuffles/lesions are exact and the C0/C1/C2/C3 arms all
consume identical information.

Fields (concat order):
  sensory 16 | h0 9 | s0 8 | t0 18 | r0 13 | o0 5   => 69 dims
"""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np

DIMS = {"sensory": 16, "h0": 9, "s0": 8, "t0": 18, "r0": 13, "o0": 5}
BUNDLE_DIM = sum(DIMS.values())  # 69
FIELDS = tuple(DIMS.keys())


@dataclass
class OrganSignals:
    sensory: np.ndarray   # raw obs (16)
    h0: np.ndarray        # internal(5) + urgency(1) + emergency(1) + drive_emb(2)
    s0: np.ndarray        # err decomposition + confidence + attribution
    t0: np.ndarray        # elapsed_hat + conf + embedding(14)
    r0: np.ndarray        # retrieved payload(8) + avail + dir + summary(3)
    o0: np.ndarray        # reserved shadow slot (zeros in v1)

    @classmethod
    def zeros(cls) -> "OrganSignals":
        return cls(**{k: np.zeros(v, dtype=np.float32) for k, v in DIMS.items()})

    def concat(self) -> np.ndarray:
        return np.concatenate([getattr(self, k) for k in FIELDS]).astype(np.float32)

    def copy(self) -> "OrganSignals":
        return OrganSignals(**{k: getattr(self, k).copy() for k in FIELDS})

    def lesioned(self, field: str) -> "OrganSignals":
        out = self.copy()
        getattr(out, field)[:] = 0.0
        return out

    def with_field(self, field: str, value: np.ndarray) -> "OrganSignals":
        out = self.copy()
        getattr(out, field)[:] = value
        return out


class Organ:
    """Minimal organ interface: per-episode reset, per-step update."""

    name: str = "organ"

    def reset(self, rng: np.random.Generator) -> None: ...
    def signal(self) -> np.ndarray: ...
