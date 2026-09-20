"""A3.1 §13-§16, §36-§38: fly-calibrated physiology overlay v1.

Distinct from the A3 generic overlay: profiles are Drosophila cell
*classes* carrying fly channel families (Para/Shab/Shaker/Shal) on
named compartments — including the runtime-derived
AXON_INITIATION_ZONE — plus a receptor profile key used by the
sign model (§27-§32) and a calibration target key used by
``calibrate.py``.

Nothing here is a direct measurement: kinetics windows are
LITERATURE_PRIOR, densities are MODEL_INFERENCE starting points that
``calibrate.py`` may replace with FITTED records (still
MODEL_INFERENCE, §23).
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pandas as pd

from ..anatomy.provenance import Provenance
from .fly_channels import FLY_CHANNEL_MODEL_VERSION, FLY_MODEL_NOTE

OVERLAY_VERSION_FLY = "physio-overlay-fly-v1"
AIS = "AXON_INITIATION_ZONE"

_S = ("soma_axon_boundary AIS enrichment factor is a model prior, "
      "not measured")
_FLY_SRC = ("Drosophila channel-family biophysics summaries; " +
            FLY_MODEL_NOTE)

# profile -> membrane + channel densities (starting points;
# calibration writes fitted values with fit metadata)
FLY_PROFILES: dict[str, dict] = {
    "kenyon_cell": {
        "receptors": "kenyon_cell",
        "membrane": {"Cm": 1.0, "g_leak": 0.25, "Ra": 100.0,
                     "E_leak": -62.0, "V_rest": -62.0,
                     "V_threshold": -45.0, "V_reset": -62.0},
        "channels": {
            "para_Na":   {"g": 40.0, "e": 50.0,
                          "comps": ["SOMA", AIS]},
            "shab_K":    {"g": 12.0, "e": -77.0,
                          "comps": ["SOMA", AIS, "AXON"]},
            "shal_K":    {"g": 25.0, "e": -77.0,
                          "comps": ["SOMA", "DENDRITE_PROX",
                                    "DENDRITE_DIST"]},
        },
    },
    "projection_neuron": {
        "receptors": "projection_neuron",
        "membrane": {"Cm": 1.0, "g_leak": 0.35, "Ra": 100.0,
                     "E_leak": -60.0, "V_rest": -60.0,
                     "V_threshold": -45.0, "V_reset": -60.0},
        "channels": {
            "para_Na":   {"g": 55.0, "e": 50.0,
                          "comps": ["SOMA", AIS]},
            "shab_K":    {"g": 16.0, "e": -77.0,
                          "comps": ["SOMA", AIS, "AXON"]},
            "shal_K":    {"g": 12.0, "e": -77.0,
                          "comps": ["SOMA", "DENDRITE_PROX",
                                    "DENDRITE_DIST"]},
        },
    },
    "descending": {
        "receptors": "descending",
        "membrane": {"Cm": 1.0, "g_leak": 0.35, "Ra": 100.0,
                     "E_leak": -58.0, "V_rest": -58.0,
                     "V_threshold": -45.0, "V_reset": -58.0},
        "channels": {
            "para_Na":   {"g": 60.0, "e": 50.0,
                          "comps": ["SOMA", AIS]},
            "shab_K":    {"g": 20.0, "e": -77.0,
                          "comps": ["SOMA", AIS, "AXON"]},
            "shaker_K":  {"g": 15.0, "e": -77.0,
                          "comps": ["SOMA", "DENDRITE_PROX"]},
        },
    },
    "motor": {
        "receptors": "motor",
        "membrane": {"Cm": 1.0, "g_leak": 0.4, "Ra": 100.0,
                     "E_leak": -55.0, "V_rest": -55.0,
                     "V_threshold": -42.0, "V_reset": -55.0},
        "channels": {
            "para_Na":   {"g": 70.0, "e": 50.0,
                          "comps": ["SOMA", AIS]},
            "shab_K":    {"g": 25.0, "e": -77.0,
                          "comps": ["SOMA", AIS, "AXON"]},
            "shaker_K":  {"g": 20.0, "e": -77.0,
                          "comps": ["SOMA", "DENDRITE_PROX"]},
        },
    },
    "sensory": {
        "receptors": "sensory",
        "membrane": {"Cm": 1.0, "g_leak": 0.3, "Ra": 100.0,
                     "E_leak": -60.0, "V_rest": -60.0,
                     "V_threshold": -45.0, "V_reset": -60.0},
        "channels": {
            "para_Na":   {"g": 50.0, "e": 50.0,
                          "comps": ["SOMA", AIS]},
            "shab_K":    {"g": 15.0, "e": -77.0,
                          "comps": ["SOMA", AIS, "AXON"]},
        },
    },
    "central_complex": {
        "receptors": "central_complex",
        "membrane": {"Cm": 1.0, "g_leak": 0.3, "Ra": 100.0,
                     "E_leak": -60.0, "V_rest": -60.0,
                     "V_threshold": -45.0, "V_reset": -60.0},
        "channels": {
            "para_Na":   {"g": 55.0, "e": 50.0,
                          "comps": ["SOMA", AIS]},
            "shab_K":    {"g": 18.0, "e": -77.0,
                          "comps": ["SOMA", AIS, "AXON"]},
            "shal_K":    {"g": 15.0, "e": -77.0,
                          "comps": ["SOMA", "DENDRITE_PROX",
                                    "DENDRITE_DIST"]},
        },
    },
    "modulatory": {
        "receptors": "modulatory",
        "membrane": {"Cm": 1.0, "g_leak": 0.3, "Ra": 100.0,
                     "E_leak": -58.0, "V_rest": -58.0,
                     "V_threshold": -45.0, "V_reset": -58.0},
        "channels": {
            "para_Na":   {"g": 45.0, "e": 50.0,
                          "comps": ["SOMA", AIS]},
            "shab_K":    {"g": 15.0, "e": -77.0,
                          "comps": ["SOMA", AIS, "AXON"]},
        },
    },
    # §38: the generic fallback is a *fly* prior, never the A3 squid
    # table
    "generic_fly": {
        "receptors": "generic_fly",
        "membrane": {"Cm": 1.0, "g_leak": 0.3, "Ra": 100.0,
                     "E_leak": -60.0, "V_rest": -60.0,
                     "V_threshold": -45.0, "V_reset": -60.0},
        "channels": {
            "para_Na":   {"g": 50.0, "e": 50.0,
                          "comps": ["SOMA", AIS]},
            "shab_K":    {"g": 16.0, "e": -77.0,
                          "comps": ["SOMA", AIS, "AXON"]},
        },
    },
}

def _v11_profiles() -> dict:
    """A3.2 v1_1 profile table: same densities as v1 but channel
    models renamed to the *_v11 classes (slower Para kinetics), plus
    the KCa-lite adaptation current on SOMA|AIS (MODEL_INFERENCE
    density prior; calibrate.py may refit it)."""
    import copy
    out = {}
    for pid, p in FLY_PROFILES.items():
        p2 = copy.deepcopy(p)
        p2["channels"] = {}
        for cname, c in p["channels"].items():
            c2 = dict(c)
            p2["channels"][cname + "_v11"] = c2
        p2["channels"]["kca_K_v11"] = {
            "g": 9.0, "e": -77.0, "comps": ["SOMA", AIS]}
        out[pid] = p2
    return out


#: A3.2 hardened profiles — v1 table above stays untouched
FLY_PROFILES_V11 = _v11_profiles()
OVERLAY_VERSION_FLY_V11 = "physio-overlay-fly-v1_1"
KCA_NOTE = ("KCa-lite spike-triggered adaptation current — "
            "MODEL_INFERENCE, not a molecular KCa fit (A3.2 §7-8)")

#: entity → profile assignment, coarse dataset-class driven (§37
#: fallback chain: cell_type keyword → super_class → nt class →
#: generic_fly)
def fly_profile_for(nt_top, super_class, cell_type=None) -> tuple[
        str, str]:
    ct = str(cell_type or "").lower()
    if "kc" in ct or "kenyon" in ct:
        return "kenyon_cell", Provenance.MODEL_INFERENCE
    if "pn" in ct or "projection" in ct:
        return "projection_neuron", Provenance.MODEL_INFERENCE
    sc = str(super_class or "").lower()
    nt = str(nt_top or "").lower()
    if nt in ("nan", "none"):
        nt = ""
    # modulatory NT wins over coarse super_class — a dopaminergic CX
    # neuron is modulatory regardless of where it sits
    if nt in ("dopamine", "serotonin", "octopamine", "tyramine",
              "histamine"):
        return "modulatory", Provenance.MODEL_INFERENCE
    if "sensory" in sc:
        return "sensory", Provenance.MODEL_INFERENCE
    if "descending" in sc:
        return "descending", Provenance.MODEL_INFERENCE
    if "motor" in sc:
        return "motor", Provenance.MODEL_INFERENCE
    if "central" in sc or "cx" in sc:
        return "central_complex", Provenance.MODEL_INFERENCE
    if nt in ("acetylcholine", "gaba", "glutamate"):
        # no finer annotation → generic_fly, NOT a guessed class
        return "generic_fly", Provenance.GENERIC_FALLBACK
    return "generic_fly", Provenance.GENERIC_FALLBACK


def _profiles_for(variant: str) -> dict:
    if variant == "v1":
        return FLY_PROFILES
    if variant == "v1_1":
        return FLY_PROFILES_V11
    raise ValueError(f"overlay variant {variant!r}")


def _membrane_rows(profiles=None) -> list[dict]:
    units = {"Cm": "uF/cm2", "g_leak": "mS/cm2", "Ra": "ohm*cm",
             "E_leak": "mV", "V_rest": "mV", "V_threshold": "mV",
             "V_reset": "mV"}
    rows = []
    for pid, prof in (profiles or FLY_PROFILES).items():
        for pname, val in prof["membrane"].items():
            prov = (Provenance.LITERATURE_PRIOR
                    if pname in ("Cm", "g_leak", "Ra")
                    else Provenance.MODEL_INFERENCE)
            rows.append({"profile_id": pid, "param": pname,
                         "value": val, "unit": units[pname],
                         "provenance": prov, "source": _FLY_SRC,
                         "confidence": 0.4})
    return rows


def _channel_rows(profiles=None) -> list[dict]:
    rows = []
    for pid, prof in (profiles or FLY_PROFILES).items():
        for cname, c in prof["channels"].items():
            rows.append({"profile_id": pid, "channel": cname,
                         "g_density": c["g"], "E_rev_mV": c["e"],
                         "compartments": "|".join(c["comps"]),
                         "provenance": Provenance.MODEL_INFERENCE,
                         "source": _FLY_SRC + "; " + _S,
                         "confidence": 0.3})
    return rows


def build_fly_overlay(entities: pd.DataFrame, anatomy_manifest: dict,
                      out_dir, fitted: dict | None = None,
                      variant: str = "v1") -> dict:
    """``fitted``: optional {profile_id: {param: record}} from
    calibrate.py — written with MODEL_INFERENCE/fit metadata (§23).
    ``variant`` "v1" | "v1_1" selects the channel-model set."""
    profiles = _profiles_for(variant)
    out = Path(out_dir)
    out.mkdir(parents=True, exist_ok=True)
    mem_rows = _membrane_rows(profiles)
    ch_rows = _channel_rows(profiles)
    # fitted channel-density knob → channel row name (§23: fitted
    # stays MODEL_INFERENCE with fit metadata, never upgraded)
    g2ch = {"g_para": "para_Na", "g_shab": "shab_K",
            "g_shal": "shal_K", "g_shaker": "shaker_K",
            "g_kca": "kca_K"}
    suffix = "_v11" if variant == "v1_1" else ""
    if fitted:
        for pid, params in fitted.items():
            for pname, rec in params.items():
                if pname in g2ch:
                    cname = g2ch[pname] + suffix
                    cspec = profiles[pid]["channels"].get(cname)
                    ch_rows = [r for r in ch_rows if not (
                        r["profile_id"] == pid
                        and r["channel"] == cname)]
                    ch_rows.append({
                        "profile_id": pid, "channel": cname,
                        "g_density": rec["value"],
                        "E_rev_mV": (cspec["e"] if cspec is not None
                                     else -77.0),
                        # fitted density for a channel the profile
                        # lacks → add at the standard active locus
                        "compartments": ("|".join(cspec["comps"])
                                         if cspec is not None
                                         else f"SOMA|{AIS}"),
                        "provenance": Provenance.MODEL_INFERENCE,
                        "source": rec["source"], "confidence": 0.5,
                        "fit": json.dumps(rec.get("fit", {}))})
                    continue
                row = {"profile_id": pid, "param": pname,
                       "value": rec["value"], "unit": rec["unit"],
                       "provenance": Provenance.MODEL_INFERENCE,
                       "source": rec["source"], "confidence": 0.5,
                       "fit": json.dumps(rec.get("fit", {}))}
                mem_rows = [r for r in mem_rows if not (
                    r["profile_id"] == pid and r["param"] == pname)]
                mem_rows.append(row)
    pd.DataFrame(mem_rows).to_parquet(out / "profiles_fly_v1.parquet",
                                      index=False)
    pd.DataFrame(ch_rows).to_parquet(out / "channels_fly_v1.parquet",
                                     index=False)
    ep = [{"entity_idx": int(r.entity_idx), "profile_id": pid,
           "assignment_provenance": prov,
           "receptor_profile": profiles[pid]["receptors"]}
          for r in entities.itertuples(index=False)
          for pid, prov in [fly_profile_for(
              getattr(r, "nt_top", None),
              getattr(r, "super_class", None),
              getattr(r, "cell_type", None))]]
    pd.DataFrame(ep).to_parquet(
        out / "entity_profiles_fly_v1.parquet", index=False)

    def sha(p):
        h = hashlib.sha256()
        h.update(Path(p).read_bytes())
        return h.hexdigest()

    manifest = {
        "overlay_version": (OVERLAY_VERSION_FLY if variant == "v1"
                            else OVERLAY_VERSION_FLY_V11),
        "channel_model_version": (FLY_CHANNEL_MODEL_VERSION
                                  if variant == "v1"
                                  else "flychan-v1_1"),
        "anatomy_manifest_hash": anatomy_manifest.get("manifest_hash"),
        "files": {p.name: sha(p) for p in sorted(out.glob("*.parquet"))},
        "profile_classes": sorted(profiles),
        "note": "fly channel families; densities are priors pending "
                "calibration; no squid-HH fallback (§38)"
                + ("; v1_1 adds KCa-lite adaptation (MODEL_INFERENCE)"
                   if variant == "v1_1" else ""),
    }
    (out / "overlay_manifest.json").write_text(json.dumps(manifest,
                                                        indent=2))
    return manifest


def load_fly_overlay(overlay_dir, anatomy_manifest: dict | None = None):
    d = Path(overlay_dir)
    manifest = json.loads((d / "overlay_manifest.json").read_text())
    if anatomy_manifest is not None:
        want = manifest.get("anatomy_manifest_hash")
        got = anatomy_manifest.get("manifest_hash")
        if want is not None and got is not None and want != got:
            raise ValueError("fly overlay anatomy mismatch")
    return {
        "manifest": manifest,
        "profiles": pd.read_parquet(d / "profiles_fly_v1.parquet"),
        "channels": pd.read_parquet(d / "channels_fly_v1.parquet"),
        "entity_profiles": pd.read_parquet(
            d / "entity_profiles_fly_v1.parquet"),
    }


def resolve_fly_entity_params(overlay: dict) -> tuple[
        dict[int, dict], dict[int, dict]]:
    """→ (entity_params, receptor_maps). Same record shape as the A3
    resolver plus the per-entity receptor profile."""
    prof = overlay["profiles"].set_index(["profile_id", "param"])
    chans = overlay["channels"]
    params, receptors = {}, {}
    for r in overlay["entity_profiles"].itertuples(index=False):
        pid = r.profile_id
        mem = {}
        for pname in ("Cm", "g_leak", "Ra", "E_leak", "V_rest",
                      "V_threshold", "V_reset"):
            key = (pid, pname)
            if key in prof.index:
                row = prof.loc[key]
                mem[pname] = {"value": float(row["value"]),
                              "unit": row["unit"],
                              "provenance": row["provenance"],
                              "source": row["source"],
                              "confidence": float(row["confidence"])}
            else:
                mem[pname] = {"value": float("nan"), "unit": "",
                              "provenance": Provenance.UNKNOWN,
                              "source": "", "confidence": 0.0}
        ch_list = [{"channel": c.channel,
                    "g_density": float(c.g_density),
                    "E_rev_mV": float(c.E_rev_mV),
                    "compartments": c.compartments.split("|"),
                    "provenance": c.provenance, "source": c.source,
                    "confidence": float(c.confidence)}
                   for c in chans[chans["profile_id"] == pid]
                   .itertuples(index=False)]
        params[int(r.entity_idx)] = {
            "profile_id": pid,
            "assignment_provenance": r.assignment_provenance,
            "membrane": mem, "channels": ch_list}
        from .receptors import receptor_map_for
        receptors[int(r.entity_idx)] = receptor_map_for(
            getattr(r, "receptor_profile", pid) or pid)
    return params, receptors
