"""FBA0Adapter: the founder substrate behind the contract.

FBA0 stays the M-series ancestor — this test pins what the adapter
exposes, including that the phenotype ``base`` record is byte-identical
to the historical ``FBA0_REFERENCE``.
"""
from __future__ import annotations

from experiments.mioba.fba.fba0 import FBA0_REFERENCE
from experiments.mioba.substrate.fba0 import (FBA0_REGIONS, FBA0Adapter,
                                              SUBSTRATE_ID)
from experiments.mioba.substrate.registry import default_registry


def test_describe_reports_identity_and_ports():
    desc = FBA0Adapter().describe()
    assert desc["substrate_id"] == SUBSTRATE_ID == "fba0"
    assert desc["kind"] == "flywire-v783-shiu-lif"
    assert desc["regions"] == list(FBA0_REGIONS)
    assert [p["endpoint"] for p in desc["ports"]] == \
        [f"fba0:{r}" for r in FBA0_REGIONS]


def test_reference_is_the_historical_fba0_record():
    """phenotype["base"] must stay byte-identical to FBA0_REFERENCE."""
    assert FBA0Adapter().reference() == dict(FBA0_REFERENCE)


def test_regions_match_the_backend_partition():
    """The adapter's region list is the backend's synthetic partition,
    in the same order — the mutation endpoint pool depends on it."""
    import pytest
    pytest.importorskip("torch")
    from experiments.mioba.fba.torch_backend import SYNTHETIC_REGIONS
    assert tuple(FBA0Adapter().regions()) == tuple(SYNTHETIC_REGIONS)


def test_port_groups_are_the_m1_mutation_policy():
    groups = FBA0Adapter().port_groups()
    assert groups["upstream"] == ["medulla", "lobula", "lobula_plate",
                                  "antennal_lobe", "optic_lobe"]
    assert groups["downstream"] == ["central_complex", "mushroom_body",
                                    "lobula_plate"]


def test_default_params_are_the_lif_defaults():
    params = FBA0Adapter().default_params()
    assert params["vThr"] == -45.0 and params["wScale"] == 0.275


def test_resolve_params_applies_genome_mutations():
    from experiments.mioba.genome.schema import ParameterMutation
    out = FBA0Adapter().resolve_params(
        [ParameterMutation(mutation_id="m", path="vThr", op="set",
                           value=200.0, scope="global")])
    assert out["vThr"] == 200.0
    assert out["tauMem"] == 20.0


def test_adapter_level_state_round_trips():
    a = FBA0Adapter()
    a.reset(seed=3)
    snap = a.snapshot()
    a.restore(snap)
    import pytest
    with pytest.raises(ValueError):
        a.restore({"substrate_id": "other"})


def test_registry_returns_fba0_adapter():
    assert type(default_registry().get("fba0")). __name__ == "FBA0Adapter"
