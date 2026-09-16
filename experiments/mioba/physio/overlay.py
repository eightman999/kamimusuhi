"""A3 §5/§6/§39: physiology parameter overlay, separated from anatomy.

The canonical anatomy store is read-only; physiology lives in
``physiology/overlays/<tag>/`` alongside it:

    profiles_v0.parquet      — profile_id, param, value, unit,
                               provenance, source, confidence,
                               compartment_scope
    channels_v0.parquet      — profile_id, channel, g_density,
                               E_rev_mV, compartments, provenance,
                               source
    entity_profiles_v0.parquet — entity_idx, profile_id,
                                 assignment_provenance
    overlay_manifest.json    — anatomy_manifest_hash, version, hashes

Resolution order (§6): neuron override → cell-type profile →
physiological profile class. v0 ships coarse classes only (§39);
nothing is a direct measurement — every value is LITERATURE_PRIOR or
GENERIC_FALLBACK and explicitly says so.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pandas as pd

from experiments.mioba.anatomy.provenance import Provenance
from experiments.mioba.physio.schema import MEMBRANE_PARAMS

OVERLAY_VERSION = "physio-overlay-v0"

# ------------------------------------------------------------------ #
# coarse physiological profile classes (§39)                           #
# All values are generic-model priors — NOT Drosophila-specific        #
# measurements. Sources are named so the honest status is visible.     #
# ------------------------------------------------------------------ #
HH_SOURCE = ("Hodgkin & Huxley 1952 (squid giant axon kinetics, "
             "reused as GENERIC model — not a Drosophila channel "
             "reconstruction)")
PROFILE_CLASSES: dict[str, dict] = {
    "generic_excitatory": {
        "description": "coarse excitatory neuron class (NT=ACh)",
        "membrane": {
            "Cm": (1.0, "uF/cm2", Provenance.LITERATURE_PRIOR, HH_SOURCE,
                   0.4),
            "g_leak": (0.3, "mS/cm2", Provenance.LITERATURE_PRIOR,
                       "generic neuron leak prior", 0.4),
            "Ra": (100.0, "ohm*cm", Provenance.LITERATURE_PRIOR,
                   "generic cytoplasm resistivity prior", 0.3),
            "E_leak": (-60.0, "mV", Provenance.MODEL_INFERENCE,
                       "chosen to place V_rest near typical "
                       "invertebrate resting levels", 0.3),
            "V_rest": (-60.0, "mV", Provenance.MODEL_INFERENCE,
                       "leak-equilibrium operating point", 0.3),
            "V_threshold": (-45.0, "mV", Provenance.MODEL_INFERENCE,
                            "spike detect level (diagnostic only)", 0.3),
            "V_reset": (-60.0, "mV", Provenance.MODEL_INFERENCE,
                        "unused in active mode; kept for schema "
                        "completeness", 0.3),
        },
        "channels": [
            {"channel": "leak", "g_density": 0.3, "E_rev_mV": -60.0,
             "compartments": ["SOMA", "DENDRITE_PROX", "DENDRITE_DIST",
                              "AXON"],
             "provenance": Provenance.LITERATURE_PRIOR,
             "source": "generic leak"},
            {"channel": "generic_Na", "g_density": 60.0,
             "E_rev_mV": 50.0,
             "compartments": ["SOMA", "AXON"],
             "provenance": Provenance.LITERATURE_PRIOR,
             "source": HH_SOURCE},
            {"channel": "generic_Kdr", "g_density": 18.0,
             "E_rev_mV": -77.0,
             "compartments": ["SOMA", "AXON"],
             "provenance": Provenance.LITERATURE_PRIOR,
             "source": HH_SOURCE},
        ],
    },
}
# other classes reuse the generic table but keep their own identity so
# future cell-type data lands in a distinct profile, not a rewrite of
# the generic one
for _name in ("generic_inhibitory", "generic_modulatory", "sensory",
              "descending", "motor"):
    PROFILE_CLASSES[_name] = {
        **PROFILE_CLASSES["generic_excitatory"],
        "description": f"coarse {_name} class (generic membrane table)",
    }

# neurotransmitter -> synaptic reversal class (§20)
NT_ESYN = {
    "acetylcholine": "exc",
    "gaba": "inh",
    "glutamate": "inh",      # GluCl in Drosophila is inhibitory
}
NT_ESYN_PROVENANCE = Provenance.MODEL_INFERENCE
UNKNOWN_NT_MODE_DEFAULT = "generic_prior"  # excitatory, audited


def _profile_for(nt_top: str | None, super_class: str | None) -> tuple[
        str, str]:
    """Coarse profile assignment. Never invents cell-specific data —
    everything is a class-level prior (MODEL_INFERENCE provenance on
    the *assignment*)."""
    sc = str(super_class or "").lower()
    if "sensory" in sc:
        return "sensory", Provenance.MODEL_INFERENCE
    if "descending" in sc:
        return "descending", Provenance.MODEL_INFERENCE
    if "motor" in sc:
        return "motor", Provenance.MODEL_INFERENCE
    nt = str(nt_top or "").lower()
    if nt in ("nan", "none"):
        nt = ""
    if nt in ("gaba", "glutamate"):
        return "generic_inhibitory", Provenance.MODEL_INFERENCE
    if nt in ("dopamine", "serotonin", "octopamine", "tyramine",
              "histamine"):
        return "generic_modulatory", Provenance.MODEL_INFERENCE
    if nt == "acetylcholine":
        return "generic_excitatory", Provenance.MODEL_INFERENCE
    return "generic_excitatory", Provenance.GENERIC_FALLBACK


def _membrane_rows() -> list[dict]:
    rows = []
    for pid, prof in PROFILE_CLASSES.items():
        for pname, (val, unit, prov, src, conf) in \
                prof["membrane"].items():
            rows.append({"profile_id": pid, "param": pname,
                         "value": val, "unit": unit,
                         "provenance": prov, "source": src,
                         "confidence": conf})
    return rows


def _channel_rows() -> list[dict]:
    rows = []
    for pid, prof in PROFILE_CLASSES.items():
        for ch in prof["channels"]:
            rows.append({"profile_id": pid, "channel": ch["channel"],
                         "g_density": ch["g_density"],
                         "E_rev_mV": ch["E_rev_mV"],
                         "compartments": "|".join(ch["compartments"]),
                         "provenance": ch["provenance"],
                         "source": ch["source"],
                         "confidence": 0.4})
    return rows


def _sha256(path: Path) -> str:
    h = hashlib.sha256()
    h.update(path.read_bytes())
    return h.hexdigest()


def build_overlay(entities: pd.DataFrame, anatomy_manifest: dict,
                  out_dir: str | Path) -> dict:
    """Write a v0 physiology overlay for the entity set. Anatomy is
    never modified — this only reads entity class labels."""
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    pd.DataFrame(_membrane_rows()).to_parquet(
        out / "profiles_v0.parquet", index=False)
    pd.DataFrame(_channel_rows()).to_parquet(
        out / "channels_v0.parquet", index=False)
    ep = [{"entity_idx": int(r.entity_idx),
           "profile_id": pid,
           "assignment_provenance": prov}
          for r in entities.itertuples(index=False)
          for pid, prov in [_profile_for(
              getattr(r, "nt_top", None),
              getattr(r, "super_class", None))]]
    pd.DataFrame(ep).to_parquet(
        out / "entity_profiles_v0.parquet", index=False)
    manifest = {
        "overlay_version": OVERLAY_VERSION,
        "anatomy_manifest_hash": anatomy_manifest.get("manifest_hash"),
        "anatomy_entities_hash":
            (anatomy_manifest.get("hashes") or {}).get("entities"),
        "files": {p.name: _sha256(p) for p in sorted(out.glob("*.parquet"))},
        "profile_classes": sorted(PROFILE_CLASSES),
        "nt_esyn_provenance": NT_ESYN_PROVENANCE,
        "unknown_nt_mode_default": UNKNOWN_NT_MODE_DEFAULT,
        "parameters": {
            "membrane": MEMBRANE_PARAMS,
            "channels": ["leak", "generic_Na", "generic_Kdr"],
        },
        "note": ("every value is LITERATURE_PRIOR/MODEL_INFERENCE/"
                 "GENERIC_FALLBACK — no direct or cell-type "
                 "measurements exist in v0"),
    }
    (out / "overlay_manifest.json").write_text(json.dumps(manifest,
                                                        indent=2))
    return manifest


def load_overlay(overlay_dir: str | Path,
                 anatomy_manifest: dict | None = None) -> dict:
    """Load an overlay; if ``anatomy_manifest`` is given, reject a
    mismatched anatomy hash unless the caller explicitly skips the
    check (§5 compatibility)."""
    d = Path(overlay_dir)
    manifest = json.loads((d / "overlay_manifest.json").read_text())
    if anatomy_manifest is not None:
        want = manifest.get("anatomy_manifest_hash")
        got = anatomy_manifest.get("manifest_hash")
        if want is not None and got is not None and want != got:
            raise ValueError(
                f"physiology overlay built for anatomy {want[:16]}… "
                f"but runtime anatomy is {got[:16]}…")
    return {
        "manifest": manifest,
        "profiles": pd.read_parquet(d / "profiles_v0.parquet"),
        "channels": pd.read_parquet(d / "channels_v0.parquet"),
        "entity_profiles": pd.read_parquet(
            d / "entity_profiles_v0.parquet"),
    }


def resolve_entity_params(overlay: dict) -> dict[int, dict]:
    """Per-entity resolved parameter set (§6 inheritance; v0 has no
    neuron-specific rows, so every entity resolves to its profile
    class — the mechanism is in place for overrides to win first)."""
    prof = overlay["profiles"].set_index(["profile_id", "param"])
    chans = overlay["channels"]
    out: dict[int, dict] = {}
    for r in overlay["entity_profiles"].itertuples(index=False):
        pid = r.profile_id
        mem = {}
        for pname in MEMBRANE_PARAMS:
            key = (pid, pname)
            if key in prof.index:
                row = prof.loc[key]
                mem[pname] = {
                    "value": float(row["value"]), "unit": row["unit"],
                    "provenance": row["provenance"],
                    "source": row["source"],
                    "confidence": float(row["confidence"])}
            else:
                mem[pname] = {
                    "value": float("nan"), "unit": "",
                    "provenance": Provenance.UNKNOWN,
                    "source": "", "confidence": 0.0}
        ch_list = []
        for c in chans[chans["profile_id"] == pid].itertuples(
                index=False):
            ch_list.append({
                "channel": c.channel, "g_density": float(c.g_density),
                "E_rev_mV": float(c.E_rev_mV),
                "compartments": c.compartments.split("|"),
                "provenance": c.provenance, "source": c.source,
                "confidence": float(c.confidence)})
        out[int(r.entity_idx)] = {
            "profile_id": pid,
            "assignment_provenance": r.assignment_provenance,
            "membrane": mem, "channels": ch_list}
    return out
