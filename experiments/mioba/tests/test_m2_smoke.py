"""M2 smoke: the departure battery on the worker evaluation path, and
substrate interchangeability — a genome can name a non-FBA0 substrate
and still develop through the same registry."""
from __future__ import annotations

import random

import pytest

from experiments.mioba.development.phenotype import develop
from experiments.mioba.fba.mock_backend import MockBackend
from experiments.mioba.fba.replicates import replicate_seeds
from experiments.mioba.genome.schema import (SubstrateGene, fba0_genome)
from experiments.mioba.substrate.base import (LesionSpec, PortSpec,
                                              SubstrateProtocol)
from experiments.mioba.substrate.registry import SubstrateRegistry
from experiments.mioba.workers.worker import evaluate_replicates


class _ProtoAdapter(SubstrateProtocol):
    """A synthetic substrate that is not FBA0: proves the architecture
    only needs the protocol, not the ancestor."""
    substrate_id = "proto0"
    kind = "synthetic-test"

    def describe(self):
        return {"substrate_id": "proto0", "kind": self.kind,
                "name": "synthetic-test-substrate"}

    def reference(self):
        return {"name": "synthetic-test-substrate", "kind": self.kind}

    def ports(self):
        return [PortSpec(name=n, kind="region", signal="event",
                         endpoint=f"substrate:proto0/{n}")
                for n in ("root", "sink")]

    def regions(self):
        return ["root", "sink"]

    def port_groups(self):
        return {"upstream": ["root"], "downstream": ["sink"]}

    def neuron_count(self):
        return 64

    def default_params(self):
        return {}

    def resolve_params(self, mutations):
        return {}

    def lesion(self, severity, n_neurons, seed):
        rng = random.Random(seed)
        k = min(int(n_neurons), max(1, int(round(n_neurons * severity))))
        return LesionSpec(substrate_id="proto0", severity=float(severity),
                          neuron_ids=sorted(
                              rng.sample(range(int(n_neurons)), k)))

    def reset(self, seed):
        pass

    def snapshot(self):
        return {}

    def restore(self, snapshot):
        pass


def test_a_genome_can_name_a_non_fba0_substrate():
    reg = SubstrateRegistry()
    reg.register("proto0", lambda gene=None: _ProtoAdapter())
    g = fba0_genome(seed=7)
    g.substrates = [SubstrateGene(substrate_id="proto0",
                                  kind="synthetic-test")]
    g = g.finalize()
    phen = develop(g, topology_mode="generic_causal", registry=reg)
    assert phen["substrates"][0]["substrate_id"] == "proto0"
    assert phen["base"]["name"] == "synthetic-test-substrate"
    assert phen["substrate_neurons"] == {"proto0": 64}
    # an organ wired on proto0's ports is functional in generic mode
    from experiments.mioba.genome.schema import (ArtificialOrgan,
                                                 Attachment)
    g.artificial_organs.append(
        ArtificialOrgan(organ_id="org_a", kind="lif_cluster", size=8))
    g.attachments += [
        Attachment(attachment_id="in", source="substrate:proto0/root",
                   target="org_a"),
        Attachment(attachment_id="out", source="org_a",
                   target="substrate:proto0/sink")]
    phen = develop(g.finalize(), topology_mode="generic_causal",
                   registry=reg)
    assert phen["structure"]["organs"]["org_a"] == "functional"


def test_m2_smoke_config_runs_the_battery():
    """The worker path attaches departure metrics when the config asks."""
    import yaml
    from pathlib import Path
    cfg = yaml.safe_load(
        (Path(__file__).parent.parent
         / "configs" / "m2_smoke.yaml").read_text())
    genome = fba0_genome(seed=11)
    phenotype = develop(genome, base_neurons=500)
    job = {"seed": 777, "replicates": 4, "duration_ms": 100,
           "environment_id": "synthetic-quiet-v0", "config": cfg}
    backend = MockBackend(n_neurons=500)
    rep = evaluate_replicates(backend, phenotype, job, "cpu",
                              execution_batch=2)
    dep = rep["summary"]["departure"]
    assert dep["enabled"] and dep["substrate_id"] == "fba0"
    assert "sham" in dep["conditions"]
    assert dep["sham_score"] == dep["intact_score"]
    assert set(dep["fba0_lesion_loss"]) == {"0.10", "0.25"}


def test_m1_smoke_config_has_no_departure_block():
    """The M1 path is untouched: no functional_departure key, no
    departure metrics in the summary."""
    import yaml
    from pathlib import Path
    cfg = yaml.safe_load(
        (Path(__file__).parent.parent
         / "configs" / "smoke_mock.yaml").read_text())
    phenotype = develop(fba0_genome(seed=1), base_neurons=500)
    job = {"seed": 5, "replicates": 2, "duration_ms": 50,
           "environment_id": "synthetic-quiet-v0", "config": cfg}
    rep = evaluate_replicates(MockBackend(n_neurons=500), phenotype, job,
                              "cpu", execution_batch=2)
    assert "departure" not in rep["summary"]
