"""Organ IR: typed ports and internal structure on organs, event-port
discipline on attachments — all additive to the M1 phenotype."""
from __future__ import annotations

from experiments.mioba.development.phenotype import develop
from experiments.mioba.genome.organ_ir import (attachment_signal,
                                               organ_ir, organ_ports,
                                               signal_known)
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             fba0_genome)


def _genome() -> object:
    g = fba0_genome(seed=6)
    g.artificial_organs.append(ArtificialOrgan(
        organ_id="org_a", kind="lif_cluster", size=16,
        ports={"in": {"direction": "input", "signal": "event"},
               "out": {"direction": "output", "signal": "event"},
               "mod": {"direction": "input", "signal": "continuous"}},
        internal={"connectivity": 0.1},
        state={"refrac": {"dtype": "f32", "shape": ["size"]}}))
    g.attachments += [
        Attachment(attachment_id="in", source="fba0:medulla",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="fba0:central_complex")]
    return g.finalize()


def test_lif_cluster_has_the_canonical_m1_ports():
    o = ArtificialOrgan(organ_id="x", kind="lif_cluster", size=4)
    ports = organ_ports(o)
    assert ports["in"]["signal"] == "event"
    assert ports["out"]["direction"] == "output"


def test_declared_ports_win_over_defaults():
    g = _genome()
    organ = g.artificial_organs[0]
    ports = organ_ports(organ)
    assert "mod" in ports and ports["mod"]["signal"] == "continuous"


def test_every_m1_edge_is_an_event_port():
    assert attachment_signal(Attachment(attachment_id="a", source="fba0",
                                        target="org_a")) == "event"
    assert signal_known("event") and signal_known("continuous")


def test_phenotype_carries_the_ir():
    phen = develop(_genome(), base_neurons=1000)
    organ = phen["artificial_organs"][0]
    assert organ["ports"]["mod"]["signal"] == "continuous"
    assert organ["internal"] == {"connectivity": 0.1}
    assert organ["state"]["refrac"]["dtype"] == "f32"
    assert phen["attachments"][0]["signal"] == "event"


def test_organ_ir_view_is_serialisable():
    import json
    ir = organ_ir(_genome().artificial_organs[0])
    json.dumps(ir)
    assert ir["kind"] == "lif_cluster"
