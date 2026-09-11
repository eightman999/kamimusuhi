"""Replicates vs execution batch.

``evaluation.replicates`` (scientific) is how many independent stochastic
realisations of one genome an evaluation averages over. The worker's
``execution_batch`` (operational) is how many of those lanes a GPU
simulates at once. Replicate ``i`` always draws from the stream seeded by
``replicate_seed(evaluation_seed, i)``, so its trajectory does not depend
on which GPU ran it, which chunk it landed in, or the chunk width.
"""
from __future__ import annotations

import hashlib

_MASK63 = (1 << 63) - 1


def replicate_seed(evaluation_seed: int, index: int) -> int:
    h = hashlib.blake2b(f"{int(evaluation_seed)}:{int(index)}".encode(),
                        digest_size=8).digest()
    return int.from_bytes(h, "big") & _MASK63


def replicate_seeds(evaluation_seed: int, replicates: int) -> list[int]:
    return [replicate_seed(evaluation_seed, i) for i in range(int(replicates))]


def chunk_indices(replicates: int, execution_batch: int) -> list[list[int]]:
    """[[0..eb-1], [eb..2eb-1], ...] covering range(replicates)."""
    eb = max(1, int(execution_batch))
    idx = list(range(int(replicates)))
    return [idx[i:i + eb] for i in range(0, len(idx), eb)]
