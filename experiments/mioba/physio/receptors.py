"""A3.1 §27-§32: receptor-aware synaptic sign model.

The sign of a synapse is NOT decided by the presynaptic transmitter
alone. Effect = f(pre transmitter, post receptor profile):

    acetylcholine + nAChR-like     → excitatory
    gaba         + Rdl/GABA-A-like → inhibitory
    glutamate    + GluCl-like      → inhibitory
    glutamate    + iGluR-like      → excitatory
    <nt>         + <no receptor>   → UNKNOWN_SIGN  (§29: Glu is never
                                                silently inhibitory)

Per-post-entity receptor profiles are coarse priors
(LITERATURE_PRIOR where the receptor biology is well documented for
the class, MODEL_INFERENCE elsewhere). Missing receptor →
``UNKNOWN_SIGN`` handled by ``unknown_sign_mode``:

    strict   — edge skipped, counted
    drop     — edge skipped (alias of strict for edge fate)
    neutral  — edge kept as excitatory conductance, audited UNKNOWN
    prior    — NT-level prior where defined (ACh exc, GABA inh);
               glutamate deliberately has NO prior (§29)
"""
from __future__ import annotations

from ..anatomy.provenance import Provenance

RECEPTOR_MODEL_VERSION = "receptor-v0"
UNKNOWN_SIGN_MODES = ("strict", "drop", "neutral", "prior")

#: NT-level prior used ONLY under unknown_sign_mode="prior";
#: glutamate intentionally absent (§29)
NT_SIGN_PRIOR = {"acetylcholine": "exc", "gaba": "inh"}

#: per fly profile class: nt_lower → receptor record
RECEPTOR_PROFILES: dict[str, dict[str, dict]] = {
    "kenyon_cell": {
        "acetylcholine": {"family": "nAChR-like", "sign": "exc",
                          "provenance": Provenance.LITERATURE_PRIOR},
        "gaba": {"family": "Rdl-like", "sign": "inh",
                 "provenance": Provenance.LITERATURE_PRIOR},
        "glutamate": {"family": "GluCl-like", "sign": "inh",
                      "provenance": Provenance.LITERATURE_PRIOR},
    },
    "projection_neuron": {
        "acetylcholine": {"family": "nAChR-like", "sign": "exc",
                          "provenance": Provenance.LITERATURE_PRIOR},
        "gaba": {"family": "Rdl-like", "sign": "inh",
                 "provenance": Provenance.LITERATURE_PRIOR},
        "glutamate": {"family": "GluCl-like", "sign": "inh",
                      "provenance": Provenance.LITERATURE_PRIOR},
    },
    "motor": {
        "acetylcholine": {"family": "nAChR-like", "sign": "exc",
                          "provenance": Provenance.LITERATURE_PRIOR},
        "gaba": {"family": "Rdl-like", "sign": "inh",
                 "provenance": Provenance.LITERATURE_PRIOR},
        # motor neurons can carry excitatory iGluR — deliberately NOT
        # lumped into GluCl (§29)
        "glutamate": {"family": "iGluR-like", "sign": "exc",
                      "provenance": Provenance.MODEL_INFERENCE},
    },
    "sensory": {
        "acetylcholine": {"family": "nAChR-like", "sign": "exc",
                          "provenance": Provenance.LITERATURE_PRIOR},
        "gaba": {"family": "Rdl-like", "sign": "inh",
                 "provenance": Provenance.LITERATURE_PRIOR},
    },
    "descending": {
        "acetylcholine": {"family": "nAChR-like", "sign": "exc",
                          "provenance": Provenance.LITERATURE_PRIOR},
        "gaba": {"family": "Rdl-like", "sign": "inh",
                 "provenance": Provenance.LITERATURE_PRIOR},
        "glutamate": {"family": "GluCl-like", "sign": "inh",
                      "provenance": Provenance.MODEL_INFERENCE},
    },
    "central_complex": {
        "acetylcholine": {"family": "nAChR-like", "sign": "exc",
                          "provenance": Provenance.LITERATURE_PRIOR},
        "gaba": {"family": "Rdl-like", "sign": "inh",
                 "provenance": Provenance.LITERATURE_PRIOR},
        "glutamate": {"family": "GluCl-like", "sign": "inh",
                      "provenance": Provenance.MODEL_INFERENCE},
    },
    "modulatory": {
        "acetylcholine": {"family": "nAChR-like", "sign": "exc",
                          "provenance": Provenance.LITERATURE_PRIOR},
        "gaba": {"family": "Rdl-like", "sign": "inh",
                 "provenance": Provenance.LITERATURE_PRIOR},
    },
    "generic_fly": {
        "acetylcholine": {"family": "nAChR-like", "sign": "exc",
                          "provenance": Provenance.LITERATURE_PRIOR},
        "gaba": {"family": "Rdl-like", "sign": "inh",
                 "provenance": Provenance.LITERATURE_PRIOR},
        # no glutamate receptor declared → Glu edges resolve UNKNOWN
    },
}


def receptor_map_for(profile_id: str) -> dict:
    return RECEPTOR_PROFILES.get(profile_id,
                                 RECEPTOR_PROFILES["generic_fly"])


def resolve_sign(nt: str | None, receptors: dict,
                 unknown_sign_mode: str = "neutral") -> tuple[
                     str | None, str, str]:
    """→ (sign, provenance, note). ``sign`` in {exc, inh, None}.

    None means the edge must be dropped (strict/drop, or prior with
    no entry — still reported)."""
    nt = (nt or "").strip().lower()
    rec = receptors.get(nt)
    if rec is not None:
        return rec["sign"], Provenance(rec["provenance"]).value, \
            rec["family"]
    # no receptor resolved for this transmitter
    if unknown_sign_mode in ("strict", "drop"):
        return None, Provenance.UNKNOWN.value, "UNKNOWN_SIGN"
    if unknown_sign_mode == "prior" and nt in NT_SIGN_PRIOR:
        return NT_SIGN_PRIOR[nt], Provenance.MODEL_INFERENCE.value, \
            "nt_prior_no_receptor"
    # neutral (or prior-without-entry): keep, excitatory, UNKNOWN flag
    return "exc", Provenance.UNKNOWN.value, "UNKNOWN_SIGN"
