"""Per-purpose seed derivation (M1 §2.4-6).

One evaluation must give the same result no matter how it was scheduled:
which worker claimed it, which device ran it, how many lanes were in the
execution batch, and how many evaluations the worker was running
concurrently. That holds only if nothing in the evaluation draws from a
stream that anything *else* also draws from — so there is no global RNG
anywhere in the evaluation path, and every stochastic component gets its
own stream derived from the evaluation seed:

    evaluation_seed  (= the job's seed, from the genome)
        |- replicate   i        per-lane Poisson drive (fba/replicates.py)
        |- environment          which neurons the environment drives
        |- noise       i        sensor noise on lane i
        |- disturbance i        when disturbances fire on lane i, and which
        \\- structure            artificial wiring of this genome

Derivations are pure functions of ``(purpose, seed, index)``, so a
replicate's stream is a property of the recording, not of the run: the
same job re-run on another GPU, at another slot count, or in another
chunk width, draws the same numbers.

``mutation_seed`` is deliberately *not* here: it belongs to the
coordinator's population RNG, whose state is checkpointed with the
lineage, not to any single evaluation.
"""
from __future__ import annotations

import hashlib

_MASK63 = (1 << 63) - 1

# purposes that may draw randomness inside one evaluation
PURPOSES = ("replicate", "environment", "noise", "disturbance",
            "structure", "lesion")


def derive(evaluation_seed: int, purpose: str, index: int = 0) -> int:
    """A 63-bit seed for ``purpose`` under ``evaluation_seed``.

    The purpose is part of the hashed string, so two purposes never
    collide even at the same index — and none of them can collide with
    ``fba.replicates.replicate_seed``, whose input has no purpose prefix.
    """
    if purpose not in PURPOSES:
        raise ValueError(f"unknown seed purpose {purpose!r}; "
                         f"expected one of {PURPOSES}")
    h = hashlib.blake2b(
        f"{purpose}:{int(evaluation_seed)}:{int(index)}".encode(),
        digest_size=8).digest()
    return int.from_bytes(h, "big") & _MASK63


def environment_seed(evaluation_seed: int) -> int:
    """Stream for the environment's own choices (which neurons are
    driven, how the episode is laid out). Shared by every replicate: the
    replicates are repeats *in the same environment*."""
    return derive(evaluation_seed, "environment")


def noise_seed(evaluation_seed: int, replicate_index: int = 0) -> int:
    """Sensor-noise stream, independent per replicate."""
    return derive(evaluation_seed, "noise", replicate_index)


def disturbance_seed(evaluation_seed: int, replicate_index: int = 0) -> int:
    """Disturbance schedule stream (when, which, how severe), independent
    per replicate so that replicates sample different crises rather than
    repeating one."""
    return derive(evaluation_seed, "disturbance", replicate_index)


def structure_seed(evaluation_seed: int) -> int:
    """Stream the genome's artificial wiring is drawn from."""
    return derive(evaluation_seed, "structure")


def lesion_seed(evaluation_seed: int, lesion_index: int = 0) -> int:
    """Lesion-mask stream for functional-departure evaluation (M2 §16):
    which substrate neurons a severity-p lesion silences. Independent
    per lesion index, and never shared with replicate/environment/noise
    streams — a lesion's draw is a property of the condition, not the
    scheduler."""
    return derive(evaluation_seed, "lesion", lesion_index)


def seed_manifest(evaluation_seed: int, replicates: int) -> dict:
    """Every stream an evaluation will use, for the research record."""
    from .replicates import replicate_seeds

    return {
        "evaluation_seed": int(evaluation_seed),
        "environment_seed": environment_seed(evaluation_seed),
        "structure_seed": structure_seed(evaluation_seed),
        "replicate_seeds": replicate_seeds(evaluation_seed, replicates),
        "noise_seeds": [noise_seed(evaluation_seed, i)
                        for i in range(int(replicates))],
        "disturbance_seeds": [disturbance_seed(evaluation_seed, i)
                              for i in range(int(replicates))],
    }
