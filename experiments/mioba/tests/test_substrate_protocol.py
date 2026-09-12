"""Substrate contract: the protocol surface every substrate
implementation must provide, and the registry that resolves substrate
genes to adapters."""
from __future__ import annotations

import pytest

from experiments.mioba.substrate.base import (PORT_SIGNALS, LesionSpec,
                                              PortSpec, check_substrate)
from experiments.mioba.substrate.registry import (SubstrateRegistry,
                                                  UnknownSubstrate,
                                                  adapter_for,
                                                  default_registry,
                                                  substrate_ids_of)
from experiments.mioba.genome.schema import fba0_genome


def test_fba0_adapter_satisfies_the_protocol():
    adapter = default_registry().get("fba0")
    assert check_substrate(adapter) == []
    assert adapter.substrate_id == "fba0"
    assert adapter.kind == "flywire-v783-shiu-lif"


def test_default_registry_knows_the_founder_substrate():
    reg = default_registry()
    assert "fba0" in reg.known()
    assert adapter_for(type("G", (), {"substrate_id": "fba0"})()) \
        .substrate_id == "fba0"


def test_unknown_substrate_is_a_loud_error_not_a_fallback():
    with pytest.raises(UnknownSubstrate):
        default_registry().get("no-such-substrate")


def test_protocol_check_catches_a_minimal_impostor():
    class NotASubstrate:
        substrate_id = "fake"
        kind = "fake"
    missing = check_substrate(NotASubstrate())
    assert "describe" in missing and "lesion" in missing


def test_a_genome_without_substrate_genes_means_fba0():
    """Legacy genomes carry no ``substrates`` list: the ancestral FBA0
    substrate is implied, never absent."""
    g = fba0_genome(seed=1)
    g.substrates = []
    assert substrate_ids_of(g) == ["fba0"]


def test_lesion_spec_is_deterministic_per_seed():
    a = default_registry().get("fba0")
    l1 = a.lesion(0.25, 1000, seed=99)
    l2 = a.lesion(0.25, 1000, seed=99)
    l3 = a.lesion(0.25, 1000, seed=100)
    assert isinstance(l1, LesionSpec)
    assert l1.neuron_ids == l2.neuron_ids
    assert l1.neuron_ids != l3.neuron_ids
    assert len(l1.neuron_ids) == 250
    assert all(0 <= i < 1000 for i in l1.neuron_ids)


def test_port_signals_are_the_m1_event_ports():
    ports = default_registry().get("fba0").ports()
    assert ports and all(isinstance(p, PortSpec) for p in ports)
    assert all(p.signal == "event" for p in ports)
    assert set(PORT_SIGNALS) >= {p.signal for p in ports}
