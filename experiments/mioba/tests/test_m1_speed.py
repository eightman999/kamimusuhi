"""M1 §2: the evaluator speed work must not change what is simulated.

Every test here pins a property that the optimisation could plausibly
break — propagation arithmetic, delay-line timing, who shares the base
graph, and what the profiler reports.
"""
from __future__ import annotations

import pytest

from experiments.mioba.development.phenotype import develop
from experiments.mioba.fba import topology
from experiments.mioba.genome.schema import (ArtificialOrgan, Attachment,
                                             Genome, ParameterMutation,
                                             fba0_genome)
from experiments.mioba.perf import PhaseTimer, merge_timings

torch = pytest.importorskip("torch")
from experiments.mioba.fba.eventgraph import EventGraph  # noqa: E402
from experiments.mioba.fba.torch_backend import TorchBackend  # noqa: E402


@pytest.fixture(autouse=True)
def _clear_caches():
    topology.BASE_CACHE.clear()
    topology.ORGAN_CACHE.clear()
    topology.BASE_CACHE.hits = topology.BASE_CACHE.misses = 0
    topology.ORGAN_CACHE.hits = topology.ORGAN_CACHE.misses = 0
    yield
    topology.BASE_CACHE.clear()
    topology.ORGAN_CACHE.clear()


def _organ_genome(size: int = 32, region: str = "medulla",
                  weight_scale: float = 1.0) -> Genome:
    g = fba0_genome()
    organ = ArtificialOrgan(organ_id="org_t1", kind="lif_cluster", size=size)
    g.artificial_organs.append(organ)
    g.attachments.append(Attachment(attachment_id="att_t1",
                                    source=f"fba0:{region}",
                                    target=organ.organ_id,
                                    weight_scale=weight_scale))
    return g.finalize()


# ------------------------------------------------------------ event graph
def test_event_propagation_matches_dense_product():
    """The event path computes the same sums as torch.sparse.mm."""
    gen = torch.Generator().manual_seed(3)
    n, m, b = 64, 400, 3
    post = torch.randint(0, n, (m,), generator=gen)
    pre = torch.randint(0, n, (m,), generator=gen)
    val = torch.rand(m, generator=gen)
    graph = EventGraph.from_coo(post, pre, val, n, n)
    W = graph.to_coo().coalesce().to_sparse_csr()

    spikes = (torch.rand((b, n), generator=gen) < 0.1).float()
    dense = torch.sparse.mm(W, spikes.T).T
    out = torch.zeros((b, n))
    graph.propagate(spikes, out)
    assert torch.allclose(out, dense, atol=1e-5)

    # non-binary events are amplitude-weighted, not just counted
    amp = spikes * 2.5
    dense2 = torch.sparse.mm(W, amp.T).T
    out2 = torch.zeros((b, n))
    graph.propagate(amp, out2)
    assert torch.allclose(out2, dense2, atol=1e-5)


def test_event_propagation_no_spikes_is_zero():
    graph = EventGraph.from_coo(torch.tensor([1]), torch.tensor([0]),
                                torch.tensor([1.0]), 4, 4)
    out = torch.zeros((2, 4))
    graph.propagate(torch.zeros((2, 4)), out)
    assert out.abs().sum().item() == 0.0


def test_backend_propagate_matches_materialised_matrix():
    n = 500
    phen = develop(_organ_genome(size=16), base_neurons=n)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, synthetic_edges=2000)
    b.initialize(phen, batch_size=2, seed=4, device="cpu")
    spikes = (torch.rand((2, b.n), generator=torch.Generator().manual_seed(1))
              < 0.05).float()
    dense = torch.sparse.mm(b.W, spikes.T).T
    assert torch.allclose(b.propagate(spikes), dense, atol=1e-4)


# ------------------------------------------------------------ delay line
def test_delay_line_delivers_after_exactly_t_delay():
    """Semantics v2: a spike emitted at step t arrives at t + D,
    D = round(tDelay/dt).

    M0 delivered it at t + D + 2 (2.0 ms for a declared 1.8 ms delay),
    because it propagated the *previous* step's spikes through a D+1 slot
    buffer. That is fixed here; the change is pinned by
    ``simulator_semantics_version = 2`` rather than being bit-compatible
    with M0.
    """
    n = 3
    b = TorchBackend(synthetic=True, synthetic_neurons=n, connectivity=0.0)
    b.initialize(develop(fba0_genome(), base_neurons=n), batch_size=1,
                 seed=0, device="cpu")
    assert b.steps_delay == round(b.params["tDelay"] / b.params["dt"]) == 18
    b.W = torch.sparse_coo_tensor(torch.tensor([[1], [0]]),
                                  torch.tensor([10.0]), (n, n)) \
        .coalesce().to_sparse_csr()
    b.set_inputs({"rates_hz": {}, "silence": []})
    b.force_spikes([0])             # neuron 0 spikes from step 0 onwards
    seen = []
    for _ in range(b.steps_delay + 3):
        before = float(b.g[0, 1].item())
        b._one_step()
        seen.append(float(b.g[0, 1].item()) - before * (1 - b.params["dt"]
                                                        / b.params["tauSyn"]))
    first = next(i for i, d in enumerate(seen) if d > 1e-6)
    assert first == b.steps_delay, (first, seen)
    assert b.delay_buf.shape[1] == b.steps_delay + 1


def test_delay_steps_round_half_up_despite_float_error():
    """1.8 / 0.1 is 18.000000000000004 in binary floating point, and
    Python's round() is banker's rounding — neither may move D."""
    n = 3
    for t_delay, dt, expected in ((1.8, 0.1, 18), (1.65, 0.1, 17),
                                  (0.05, 0.1, 1), (2.0, 0.1, 20)):
        g = fba0_genome()
        g.parameter_mutations.append(ParameterMutation(
            mutation_id="d", path="tDelay", op="set", value=t_delay))
        g.parameter_mutations.append(ParameterMutation(
            mutation_id="s", path="dt", op="set", value=dt))
        b = TorchBackend(synthetic=True, synthetic_neurons=n,
                         connectivity=0.0)
        b.initialize(develop(g.finalize(), base_neurons=n), batch_size=1,
                     seed=0, device="cpu")
        assert b.steps_delay == expected, (t_delay, dt, b.steps_delay)


# ------------------------------------------------------------ residency
def test_base_graph_is_shared_across_genome_seeds():
    """M1: the FBA0 reference is sampled from fba.base_seed, so two
    individuals with different random seeds live in the same base graph
    (in M0 every genome got its own pseudo-connectome)."""
    n = 400
    phen = develop(fba0_genome(), base_neurons=n)
    a = TorchBackend(synthetic=True, synthetic_neurons=n, synthetic_edges=800)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, synthetic_edges=800)
    a.initialize(phen, batch_size=1, seed=11, device="cpu")
    b.initialize(phen, batch_size=1, seed=999, device="cpu")
    assert a.base_topology_key == b.base_topology_key
    assert torch.equal(a.W_base.row, b.W_base.row)
    assert torch.equal(a.W_base.val, b.W_base.val)
    assert topology.BASE_CACHE.hits == 1 and topology.BASE_CACHE.misses == 1

    # a different base seed is a different reference graph
    c = TorchBackend(synthetic=True, synthetic_neurons=n, synthetic_edges=800,
                     base_seed=7)
    c.initialize(phen, batch_size=1, seed=11, device="cpu")
    assert c.base_topology_key != a.base_topology_key


def test_parameter_mutation_reuses_the_resident_base():
    """A wScale child must not rebuild 14M edges: the base is cached raw
    and the scale is applied at propagation time."""
    n = 400
    base = develop(fba0_genome(), base_neurons=n)
    g = fba0_genome()
    g.parameter_mutations.append(ParameterMutation(
        mutation_id="m", path="wScale", op="scale", value=2.0))
    child = develop(g.finalize(), base_neurons=n)

    a = TorchBackend(synthetic=True, synthetic_neurons=n, synthetic_edges=800)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, synthetic_edges=800)
    a.initialize(base, batch_size=1, seed=1, device="cpu")
    b.initialize(child, batch_size=1, seed=1, device="cpu")
    assert topology.BASE_CACHE.misses == 1 and topology.BASE_CACHE.hits == 1
    assert b._base_scale == pytest.approx(2.0 * a._base_scale)

    spikes = torch.zeros((1, n))
    spikes[0, 0] = 1.0
    assert torch.allclose(b.propagate(spikes), 2.0 * a.propagate(spikes),
                          atol=1e-5)


def test_organ_topology_cache_is_keyed_by_structure_not_parameters():
    n = 400
    struct = _organ_genome(size=16)
    child = Genome.from_dict(struct.to_dict())
    child.parameter_mutations.append(ParameterMutation(
        mutation_id="m", path="vThr", op="scale", value=1.1))
    child = child.finalize()

    kw = dict(synthetic=True, synthetic_neurons=n, synthetic_edges=800)
    a, b = TorchBackend(**kw), TorchBackend(**kw)
    a.initialize(develop(struct, base_neurons=n), batch_size=1, seed=2,
                 device="cpu")
    b.initialize(develop(child, base_neurons=n), batch_size=1, seed=2,
                 device="cpu")
    assert a.organ_topology_key == b.organ_topology_key
    assert topology.ORGAN_CACHE.hits == 1

    # a structural change is a different key
    bigger = TorchBackend(**kw)
    bigger.initialize(develop(_organ_genome(size=24), base_neurons=n),
                      batch_size=1, seed=2, device="cpu")
    assert bigger.organ_topology_key != a.organ_topology_key


def test_topology_cache_can_be_disabled():
    n = 300
    phen = develop(fba0_genome(), base_neurons=n)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, synthetic_edges=600,
                     topology_cache=False)
    b.initialize(phen, batch_size=1, seed=1, device="cpu")
    assert len(topology.BASE_CACHE) == 0


# ------------------------------------------------------------ profiling
def test_phase_timer_reports_phases_and_is_free_when_disabled():
    t = PhaseTimer(enabled=True, label="cpu")
    with t.phase("simulation_loop"):
        sum(range(10000))
    d = t.to_dict()
    assert d["phases"]["simulation_loop"]["calls"] == 1
    assert d["phases"]["simulation_loop"]["seconds"] > 0
    assert d["accounted_s"] <= d["wall_s"] + 1e-6

    off = PhaseTimer(enabled=False)
    with off.phase("simulation_loop"):
        pass
    assert off.to_dict()["phases"] == {}


def test_merge_timings_sums_chunks():
    a = {"wall_s": 1.0, "phases": {"simulation_loop": {"seconds": 0.5,
                                                       "calls": 1}}}
    b = {"wall_s": 2.0, "phases": {"simulation_loop": {"seconds": 1.0,
                                                       "calls": 2},
                                   "metrics": {"seconds": 0.25, "calls": 1}}}
    m = merge_timings([a, b])
    assert m["wall_s"] == 3.0
    assert m["phases"]["simulation_loop"] == {"seconds": 1.5, "calls": 3,
                                              "pct_of_wall": 50.0}
    assert m["phases"]["metrics"]["calls"] == 1


def test_backend_initialize_records_phase_breakdown():
    n = 400
    t = PhaseTimer(enabled=True)
    b = TorchBackend(synthetic=True, synthetic_neurons=n, synthetic_edges=800)
    b.initialize(develop(_organ_genome(size=8), base_neurons=n), batch_size=1,
                 seed=1, device="cpu", timer=t)
    phases = t.to_dict()["phases"]
    for name in ("mutation_resolve", "topology_construction",
                 "host_to_device", "state_init"):
        assert name in phases, (name, sorted(phases))


# ------------------------------------------------------------ slots (§2.2)
def test_worker_state_tracks_one_job_per_slot_thread():
    """With --slots N the heartbeat must report every in-flight job:
    a worker that dies holding three must requeue three."""
    import threading

    from experiments.mioba.workers.worker import WorkerState

    st = WorkerState()
    started, done = threading.Barrier(3), threading.Event()

    def slot(job_id):
        st.set_job(job_id, "running")
        started.wait(timeout=5)
        done.wait(timeout=5)
        st.set_job(None)          # clears only this thread's job

    threads = [threading.Thread(target=slot, args=(f"job{i}",))
               for i in range(2)]
    for t in threads:
        t.start()
    started.wait(timeout=5)
    assert st.current_job_ids == ["job0", "job1"]
    assert st.current_job_id in ("job0", "job1")
    done.set()
    for t in threads:
        t.join(timeout=5)
    assert st.current_job_ids == []
    assert st.phase == "idle"


def test_heartbeat_payload_reports_all_slot_jobs():
    from experiments.mioba.workers.worker import heartbeat_payload

    p = heartbeat_payload("w1", "jobA", device="cpu",
                          current_job_ids=["jobA", "jobB"], slots=2)
    assert p["current_job_id"] == "jobA"
    assert p["current_job_ids"] == ["jobA", "jobB"]
    assert p["slots"] == 2
    # single-slot callers keep the old shape
    q = heartbeat_payload("w1", "jobA", device="cpu")
    assert q["current_job_ids"] == ["jobA"]


def test_slot_sweep_reports_evaluations_per_minute(tmp_path):
    """§2.2's figure of merit is throughput, not latency."""
    from experiments.mioba.perf.evalbench import founder_population, slot_sweep

    cfg = {"evaluation": {"backend": "mock", "duration_ms": 20,
                          "replicates": 2,
                          "environment_id": "synthetic-quiet-v0"},
           "fba": {"synthetic": True, "synthetic_neurons": 200},
           "env": {"stim_fraction": 0.02, "stim_rate_hz": 50}}
    genomes = founder_population(3)
    rep = slot_sweep(genomes, cfg, "cpu", slot_candidates=(1, 2))
    assert [r["slots"] for r in rep["rows"]] == [1, 2]
    for row in rep["rows"]:
        assert row["succeeded"] == 3 and row["failed"] == 0
        assert row["successful_evaluations_per_minute"] > 0
        assert row["timing"]["phases"]["simulation_loop"]["calls"] >= 3
    assert rep["selected_slots"] in (1, 2)


# ------------------------------------------------------ cheap eval (§2.3)
def test_rank_agreement_accepts_and_rejects_on_the_stated_criteria():
    from experiments.mioba.perf import agreement_report, spearman

    ids = [f"g{i}" for i in range(8)]
    gold = [8, 7, 6, 5, 4, 3, 2, 1]
    assert spearman(gold, gold) == pytest.approx(1.0)

    # a cheap evaluator that only swaps neighbours keeps the ranking
    good = [8, 7, 6, 5, 4, 2, 3, 1]
    rep = agreement_report(ids, gold, good, k=8)
    assert rep["accepted"] and rep["spearman_rho"] >= 0.85
    assert rep["top_k"]["overlap"] == 8

    # a reversed one must be rejected, with the numbers that rejected it
    rep = agreement_report(ids, gold, list(reversed(gold)), k=8)
    assert not rep["accepted"]
    assert rep["spearman_rho"] == pytest.approx(-1.0)
    assert any("spearman" in r for r in rep["rejected_because"])

    # no spread at all is a rejection, not a crash
    rep = agreement_report(ids, gold, [1.0] * 8, k=8)
    assert not rep["accepted"] and rep["spearman_rho"] is None


def test_average_ranks_shares_ties():
    from experiments.mioba.perf import average_ranks

    assert average_ranks([1.0, 2.0, 2.0, 4.0]) == [1.0, 2.5, 2.5, 4.0]


def test_top_k_overlap_is_deterministic_under_ties():
    from experiments.mioba.perf import top_k_overlap

    ids = ["a", "b", "c", "d"]
    flat = [1.0, 1.0, 1.0, 1.0]
    one = top_k_overlap(ids, flat, flat, 2)
    two = top_k_overlap(ids, flat, flat, 2)
    assert one == two and one["overlap"] == 2
