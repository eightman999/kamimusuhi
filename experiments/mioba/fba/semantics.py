"""Simulator semantics identity.

A recorded evaluation is only reproducible if four things agree, and
they change independently of each other:

``dataset_identity``
    *Which network.* Handled by the backend (``dataset_identity()``):
    the connectome, its version, its manifest hash, and — since M1 — the
    base seed that the shared FBA0 reference graph is sampled from.

``simulator_semantics_version``
    *What the equations do.* Bumped whenever the model's behaviour
    changes for reasons that are not a bug in the implementation of the
    documented model: M1 fixes the delay line so that a spike emitted at
    step ``t`` arrives at ``t + round(tDelay/dt)``, where M0 delivered it
    two steps later.

``rng_protocol_version``
    *Which random numbers are drawn, in which order, from which stream.*
    M1 draws one uniform per *driven* neuron per step instead of one per
    neuron, and derives per-purpose seeds (evaluation / environment /
    noise / disturbance) from the evaluation seed, so the same genome and
    seeds give the same trajectory regardless of worker, device, slot
    count or execution batch.

``propagation_backend``
    *How the synaptic sum is evaluated.* ``event_csc`` gathers only the
    edges leaving neurons that spiked; ``sparse_csr`` is the dense-vector
    product M0 used. Both compute the same sum in a different
    floating-point order.

All four are recorded on every evaluation and folded into
``scientific_config_hash``, so a semantics change forces a new experiment
id rather than silently continuing an old one. Replay compares them and
refuses on mismatch: an M0 (v0 / semantics 1) recording is **not**
re-runnable by M1 code, and must be replayed from the M0 commit.
"""
from __future__ import annotations

# 1 = M0: delay = round(tDelay/dt) + 2 steps, dense sparse.mm propagation
# 2 = M1: delay = round(tDelay/dt) steps exactly, event-driven propagation
SIMULATOR_SEMANTICS_VERSION = 2

# 1 = M0: one uniform per neuron per step; base graph from the genome seed
# 2 = M1: one uniform per driven neuron per step; per-purpose seed
#         derivation (see fba/seeds.py); shared base graph from base_seed
RNG_PROTOCOL_VERSION = 2

PROPAGATION_EVENT_CSC = "event_csc"
PROPAGATION_SPARSE_CSR = "sparse_csr"
PROPAGATION_AUTO = "auto"


class SemanticsMismatch(RuntimeError):
    """A recording was produced by a different simulator than the one
    asked to reproduce it. There is no override flag: use the commit that
    produced the recording."""


def semantics(propagation_backend: str = PROPAGATION_EVENT_CSC) -> dict:
    """The M1 torch simulator's semantics."""
    return {
        "simulator_semantics_version": SIMULATOR_SEMANTICS_VERSION,
        "rng_protocol_version": RNG_PROTOCOL_VERSION,
        "propagation_backend": propagation_backend,
    }


def unversioned_semantics(backend_name: str) -> dict:
    """Semantics of a backend that has never been revised.

    The version numbers are per backend family (the ``backend`` column
    identifies which), so the smoke-test mock — which has no delay line
    at all and its own numpy RNG — stays at 1 rather than borrowing the
    torch backend's history.
    """
    return {"simulator_semantics_version": 1, "rng_protocol_version": 1,
            "propagation_backend": backend_name}


def check_replayable(recorded: dict | None,
                     current: dict | None = None) -> list[str]:
    """Return the list of fatal differences between a recording's
    semantics and the current ones.

    A recording with *no* semantics fields predates M1 (M0 ran semantics
    1 / rng 1 / sparse_csr) and is reported as such rather than assumed
    compatible.
    """
    current = current or semantics()
    rec = dict(recorded or {})
    if not rec:
        rec = {"simulator_semantics_version": 1, "rng_protocol_version": 1,
               "propagation_backend": PROPAGATION_SPARSE_CSR,
               "_inferred": True}
    problems = []
    for key in ("simulator_semantics_version", "rng_protocol_version"):
        a, b = rec.get(key), current.get(key)
        if a != b:
            problems.append(
                f"{key}: recorded {a!r}, this build is {b!r}"
                + (" (inferred: the recording predates semantics tagging)"
                   if rec.get("_inferred") else ""))
    # the propagation backend changes only the summation order, so it is
    # reported as a parity note, not a refusal
    return problems


def assert_replayable(recorded: dict | None,
                      current: dict | None = None) -> None:
    problems = check_replayable(recorded, current)
    if problems:
        raise SemanticsMismatch(
            "; ".join(problems)
            + " - replay this recording with the commit that produced it")
