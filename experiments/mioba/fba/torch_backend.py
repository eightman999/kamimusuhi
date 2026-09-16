"""Torch FBA backend — our own implementation of the Shiu et al. (2024)
Drosophila LIF model (read equations from the GPL fly-brain repo only;
no code copied).

Model: Poisson drive (scalePoisson=250, wScale=0.275) -> recurrent
sparse weights -> alpha synapse (tauSyn=5 ms) with a 1.8 ms delay ring
buffer -> LIF (tauMem=20 ms, vRest=-52 mV, vThr=-45 mV, refractory
2.2 ms) at dt=0.1 ms.

Connectivity convention
-----------------------
Sparse CSR tensors indexed ``W[post, pre]`` (row = postsynaptic, column =
presynaptic). Propagation is

    I_post = W @ spikes_pre        i.e.  torch.sparse.mm(W, spikes.T).T

so a spike of neuron ``pre`` delivers ``W[post, pre]`` to ``post``.
(``spikes @ W`` would propagate post -> pre and, for CSR, torch returns
zeros silently — never use it.)

The graph is split in two (M1 §2.1, see fba/topology.py):

``self.W_base``   ``n_base x n_base``, the FBA0 reference. Built once per
                  worker process from ``fba.base_seed`` (not from the
                  genome seed) and kept resident on the device; its values
                  are stored *raw* and multiplied by ``self._base_scale``
                  at propagation time, so a ``wScale`` mutation never
                  rebuilds it.
``self.W_extra``  ``N x N``, artificial-organ internal edges and
                  attachment edges. Built per genome (its sampled index
                  set is cached by ``organ_topology_hash``), ``None`` when
                  the genome has no organs.

No ``N x N`` dense tensor is ever allocated, so memory is
``O(E_fba0 + E_organs + E_attachments)``.

Delay line
----------
A spike emitted at step ``t`` arrives at ``t + D`` with
``D = round(tDelay/dt)`` — 18 steps = 1.8 ms at the reference
parameters. M0 propagated the *previous* step's spikes into a
``steps_delay + 1`` slot buffer, which delivered them at ``t + D + 2``
(2.0 ms); that off-by-two has been fixed since
``simulator_semantics_version = 2``, so M0 recordings are not
bit-comparable and are refused by replay rather than re-run.

The buffer is indexed by ``self._delay_ptr`` rather than rolled: M0
called ``torch.roll`` on the whole ``(B, L, N)`` tensor every step, i.e.
copied ~10 MB per lane per step (≈50 GB of memory traffic over one
500 ms evaluation at FlyWire scale) to achieve what a pointer increment
does.

Poisson drive
-------------
``set_inputs`` keeps only the neurons whose rate is non-zero
(``self._drive_idx``). M0 drew ``N`` uniforms per lane per step and
compared them against a vector that was zero for 99% of entries; M1 draws
one uniform per *driven* neuron. Each input event adds
``wScale * scalePoisson`` (68.75 mV) to ``v`` — the reference model's
PoissonInput weight ``w_syn * f_poi``. Semantics versions <= 2 applied
``scalePoisson`` alone (a 250 mV kick); the random stream is also
consumed differently than M0, so M1 trajectories are not comparable to
M0 ones — as with the topology change, this is a new
``scientific_config_hash``.

Data
----
Connectivity is loaded from a fly-brain ``data/`` directory pointed to
by the ``MIOBA_FLY_BRAIN_DATA`` env var or config ``fba.data_dir``:
``2025_Completeness_783.csv`` (FlyWire root ids) and
``2025_Connectivity_783.parquet`` (Presynaptic_Index,
Postsynaptic_Index, "Excitatory x Connectivity" columns). pandas+pyarrow
are optional imports — without them loading raises BackendUnavailable.
COO indices/values are cached under
``<runs>/cache/weights_<sha256(size,mtime)>.pt``.

With no data dir, ``synthetic=True`` gives a random sparse N=2000
network so the backend can be exercised on CPU. In synthetic mode the
FBA0 neurons are split into equal named pseudo-regions
(``region_mode="synthetic-region-v0"``) so ``fba0:<region>`` attachments
are wired to a definite neuron range. With real data no region mapping
exists yet; an ``fba0:<region>`` attachment raises
``UnsupportedAttachmentRegion`` instead of being wired randomly.

Randomness
----------
Every random draw goes through explicit generators: ``self._build_gen``
(CPU, seeded from ``seed``) for network construction and one
``self._gens[b]`` (on ``device``) per batch lane for the per-step
Poisson drive. Lane ``b`` is seeded with ``replicate_seeds[b]``
(default ``replicate_seed(seed, b)``), so a lane's trajectory is a
function of its replicate seed only — not of the batch width or of the
other lanes. ``checkpoint()`` stores all generator states plus the
simulation state, so replaying the same steps after ``restore()`` is
bit-identical on CPU. On CUDA, sparse matmul may use non-deterministic
atomics; see README "Determinism".
"""
from __future__ import annotations

import hashlib
import json
import math
import os
import pickle
import time
from pathlib import Path

import numpy as np

from ..perf import PhaseTimer
from .backend import BackendUnavailable, FbaBackend, validate_neuron_selection
from .eventgraph import EventGraph
from .fba0 import DATA_FILES
from .params import DEFAULT_PARAMS, UnsupportedAttachmentRegion
from .replicates import replicate_seeds as _default_replicate_seeds
#: runtime modes carrying an explicit membrane equation (A3/A3.1)
ACTIVE_MODES = ("active_hh_v0", "active_fly_v1")

from .semantics import (PROPAGATION_AUTO, PROPAGATION_EVENT_CSC,
                        PROPAGATION_SPARSE_CSR, semantics)
from .topology import (BASE_CACHE, ORGAN_CACHE, base_topology_key,
                       organ_topology_hash)

try:
    import torch
except ImportError:  # pragma: no cover
    torch = None

PARAMS = dict(DEFAULT_PARAMS)

SYNTHETIC_REGION_MODE = "synthetic-region-v0"
SYNTHETIC_REGIONS = ("medulla", "lobula", "lobula_plate", "central_complex",
                     "mushroom_body", "optic_lobe", "antennal_lobe")
ORGAN_INTERNAL_P = 0.05     # organ-internal connection probability
ATTACHMENT_P = 0.01         # source-neuron x target-neuron pair probability


def _load_connectome(data_dir: Path, cache_dir: Path | None):
    """Return (n_neurons, post_idx[int64], pre_idx[int64], w[float32],
    manifest_hash). Never materialises a dense matrix."""
    try:
        import pandas as pd  # noqa: F401
        import pyarrow  # noqa: F401
    except ImportError as exc:
        raise BackendUnavailable(
            "pandas+pyarrow required to load fly-brain data") from exc

    con_path = data_dir / "2025_Connectivity_783.parquet"
    comp_path = data_dir / "2025_Completeness_783.csv"
    if not con_path.is_file():
        raise BackendUnavailable(f"missing {con_path}")

    stat = con_path.stat()
    key = hashlib.sha256(f"{stat.st_size}:{stat.st_mtime_ns}".encode()).hexdigest()[:16]
    cache_file = (cache_dir / f"weights_{key}.pt") if cache_dir else None
    if cache_file and cache_file.is_file():
        blob = torch.load(cache_file, weights_only=False)
        return blob["n"], blob["post"], blob["pre"], blob["w"], blob["manifest"]

    h = hashlib.sha256()
    for name in sorted(DATA_FILES):
        f = data_dir / name
        if f.is_file():
            h.update(name.encode())
            h.update(f.read_bytes())
    manifest = h.hexdigest()

    df = pd.read_parquet(con_path)
    col_pre = "Presynaptic_Index" if "Presynaptic_Index" in df.columns else df.columns[0]
    col_post = "Postsynaptic_Index" if "Postsynaptic_Index" in df.columns else df.columns[1]
    col_w = ("Excitatory x Connectivity"
             if "Excitatory x Connectivity" in df.columns else df.columns[-1])
    pre = torch.from_numpy(df[col_pre].to_numpy().astype(np.int64))
    post = torch.from_numpy(df[col_post].to_numpy().astype(np.int64))
    w = torch.from_numpy(df[col_w].to_numpy().astype(np.float32))
    n = int(max(int(pre.max()) if len(pre) else -1,
                int(post.max()) if len(post) else -1) + 1)
    if comp_path.is_file():
        n = max(n, sum(1 for _ in comp_path.open("rb")) - 1)
    if cache_file:
        cache_file.parent.mkdir(parents=True, exist_ok=True)
        torch.save({"n": n, "post": post, "pre": pre, "w": w,
                    "manifest": manifest}, cache_file)
    return n, post, pre, w, manifest


def _load_anatomy(anatomy_dir: Path, cache_dir: Path | None):
    """Load a canonical anatomy store (``anatomy/schema.py``).

    Returns (n_neurons, post_idx, pre_idx, w, manifest) where manifest is
    the sha256 of ``manifest.json`` — a compact, content-based identity
    for the whole store. Dataset-agnostic by construction: BANC, FAFB or
    any future importer all produce the same canonical files."""
    try:
        import pandas as pd  # noqa: F401
        import pyarrow  # noqa: F401
    except ImportError as exc:
        raise BackendUnavailable(
            "pandas+pyarrow required to load an anatomy store") from exc

    con_path = anatomy_dir / "connectivity.parquet"
    man_path = anatomy_dir / "manifest.json"
    if not con_path.is_file() or not man_path.is_file():
        raise BackendUnavailable(
            f"anatomy store {anatomy_dir} lacks connectivity.parquet "
            f"or manifest.json — run an importer first")

    manifest = json.loads(man_path.read_text())
    manifest_hash = hashlib.sha256(
        man_path.read_bytes()).hexdigest()

    stat = con_path.stat()
    key = hashlib.sha256(
        f"{manifest_hash}:{stat.st_size}:{stat.st_mtime_ns}".encode()
    ).hexdigest()[:16]
    cache_file = (cache_dir / f"anatomy_weights_{key}.pt") \
        if cache_dir else None
    if cache_file and cache_file.is_file():
        blob = torch.load(cache_file, weights_only=False)
        return blob["n"], blob["post"], blob["pre"], blob["w"], \
            manifest_hash, manifest

    df = pd.read_parquet(con_path)
    pre = torch.from_numpy(df["pre_idx"].to_numpy().astype(np.int64))
    post = torch.from_numpy(df["post_idx"].to_numpy().astype(np.int64))
    w = torch.from_numpy(df["weight"].to_numpy().astype(np.float32))
    n = int(manifest.get("n_neurons") or 0)
    n = max(n, int(max(int(pre.max()) if len(pre) else -1,
                       int(post.max()) if len(post) else -1) + 1))
    if cache_file:
        cache_file.parent.mkdir(parents=True, exist_ok=True)
        torch.save({"n": n, "post": post, "pre": pre, "w": w},
                   cache_file)
    return n, post, pre, w, manifest_hash, manifest


def _sample_edges(n_post_lo, n_post_hi, n_pre_lo, n_pre_hi, p, gen,
                  no_self=True, n_edges: int | None = None):
    """Sample ~p*|post|*|pre| (or exactly ``n_edges``) directed edges
    pre->post uniformly (with replacement; duplicates are summed by
    coalesce) as (post, pre) int64 tensors. O(edges) memory."""
    n_post = n_post_hi - n_post_lo
    n_pre = n_pre_hi - n_pre_lo
    m = int(n_edges) if n_edges is not None else int(round(p * n_post * n_pre))
    if n_post <= 0 or n_pre <= 0 or m <= 0:
        e = torch.empty(0, dtype=torch.int64)
        return e, e.clone()
    post = torch.randint(n_post_lo, n_post_hi, (m,), generator=gen)
    pre = torch.randint(n_pre_lo, n_pre_hi, (m,), generator=gen)
    if no_self:
        keep = post != pre
        post, pre = post[keep], pre[keep]
    return post, pre


def _sample_mixed(post_sel, pre_sel, p, gen, no_self=True):
    """Like ``_sample_edges`` but each side may be an (lo, hi) range or
    an explicit int64 tensor of indices (anatomical graft endpoints).
    Returns absolute (post, pre) indices, ~p*|post|*|pre| pairs."""
    n_post = (post_sel[1] - post_sel[0] if isinstance(post_sel, tuple)
              else int(post_sel.numel()))
    n_pre = (pre_sel[1] - pre_sel[0] if isinstance(pre_sel, tuple)
             else int(pre_sel.numel()))
    m = int(round(p * n_post * n_pre))
    if n_post <= 0 or n_pre <= 0 or m <= 0:
        e = torch.empty(0, dtype=torch.int64)
        return e, e.clone()
    pi = torch.randint(0, n_post, (m,), generator=gen)
    ri = torch.randint(0, n_pre, (m,), generator=gen)
    post = (pi + post_sel[0] if isinstance(post_sel, tuple)
            else post_sel[pi])
    pre = (ri + pre_sel[0] if isinstance(pre_sel, tuple)
           else pre_sel[ri])
    if no_self:
        keep = post != pre
        post, pre = post[keep], pre[keep]
    return post, pre


class TorchBackend(FbaBackend):
    name = "torch"

    def __init__(self, data_dir: str | None = None, synthetic: bool = True,
                 synthetic_neurons: int = 2000, connectivity: float = 0.01,
                 runs_dir: str | None = None, region_mode: str | None = None,
                 synthetic_edges: int | None = None, base_seed: int = 0,
                 topology_cache: bool = True,
                 propagation_backend: str = PROPAGATION_EVENT_CSC,
                 dense_above: float = 0.05,
                 anatomy_dir: str | None = None,
                 base_override: tuple | None = None,
                 voltage_coupling: tuple | None = None,
                 runtime_mode: str = "passive_lif"):
        if torch is None:
            raise BackendUnavailable("torch not installed")
        # AFC A1: a canonical anatomy store takes precedence over both
        # the FlyWire-parquet path and the synthetic fallback. The
        # backend only understands the canonical format — dataset
        # specifics live entirely in the importer.
        # A2: ``base_override`` injects a prebuilt (n, post, pre, w)
        # graph — used by the compartment-reduction path where runtime
        # nodes are (entity, compartment) pairs, not entities.
        self.anatomy_dir = (anatomy_dir
                            or os.environ.get("MIOBA_ANATOMY_DIR"))
        self.data_dir = data_dir or os.environ.get("MIOBA_FLY_BRAIN_DATA")
        self.synthetic = synthetic
        self.synthetic_neurons = int(synthetic_neurons)
        # seed of the *shared* FBA0 reference graph (M1): one base network
        # for the whole run, independent of any genome's random_seed
        self.base_seed = int(base_seed)
        self.use_topology_cache = bool(topology_cache)
        # event_csc | sparse_csr | auto (hybrid, M1 §2.4-4). "auto" keeps
        # the event path until more than `dense_above` of the presynaptic
        # population fires in one step, where the whole-matrix product
        # becomes the cheaper way to compute the same sum.
        self.propagation_backend = str(propagation_backend)
        self.dense_above_threshold = float(dense_above)
        if self.propagation_backend == PROPAGATION_SPARSE_CSR:
            self._dense_above = -1.0        # always dense
        elif self.propagation_backend == PROPAGATION_AUTO:
            self._dense_above = self.dense_above_threshold
        else:
            self._dense_above = None        # always event-driven
        # activity accumulators: they span the whole evaluation (every
        # replicate chunk on this backend instance), so lane-steps rather
        # than steps is the normaliser when chunk widths differ
        self._act_steps = self._act_lane_steps = 0
        self._act_events = self._act_edges = self._act_edges_extra = 0
        self._act_wall = 0.0
        # M1.5 §8 runaway guard: deterministic limits checked against the
        # job-spanning counters above. A tripped guard stops stepping for
        # the rest of the evaluation and is reported, never silent.
        self._run_limits: dict = {}
        self._guard: dict | None = None
        self._guard_wall_start: float | None = None
        # explicit edge count (FlyWire-scale smoke: 139k neurons / ~14M
        # edges) wins over the pair probability
        self.synthetic_edges = int(synthetic_edges) if synthetic_edges else None
        self.connectivity = float(connectivity)
        self.runs_dir = Path(runs_dir) if runs_dir else None
        # explicit region_mode wins; synthetic defaults to the pseudo-region
        # partition, real data to None (=> no fba0:<region> attachments)
        self._region_mode_arg = region_mode
        self.region_mode = region_mode or (SYNTHETIC_REGION_MODE
                                           if not self.data_dir
                                           and not self.anatomy_dir
                                           else None)
        self._manifest_hash: str | None = None
        self._anatomy_manifest: dict | None = None
        self._force: torch.Tensor | None = None
        # (n, post, pre, w) — bypasses every loader when set (A2 reduced
        # compartment graphs, tests). Values are final edge weights.
        self.base_override = base_override
        # (row, col, val) — passive voltage diffusion between
        # compartment nodes of one entity (A2). Applied each step as
        # dv_i += dt/tauCoup * Σ_j Vcoup[i,j] v_j. None → point mode.
        self.voltage_coupling = voltage_coupling
        # A3: "passive_lif" (frozen A2.1 semantics) | "active_hh_v0"
        # (explicit membrane equation + pluggable channels on the
        # nodes named by phenotype["physiology"]["active_idx"] —
        # mixed-fidelity, §25). A3.1 adds "active_fly_v1" — same
        # machinery, fly channel set + AIS emitters via the compiler.
        self.runtime_mode = str(runtime_mode)
        self._active_idx = None
        self._num_failure: dict | None = None
        self._i_ext: "torch.Tensor | None" = None
        self._trace_nodes = None
        self._trace: dict = {}
        self._base_esyn = None
        self._base_raw = None
        self._W_exc = None
        self._W_inh = None

    # ------------------------------------------------------------ identity
    def semantics(self) -> dict:
        return semantics(self.propagation_backend)

    def dataset_identity(self) -> dict:
        """Logical dataset identity for the research record (never a raw
        path): id, version, manifest hash, region mode."""
        if self.anatomy_dir:
            m = self._anatomy_manifest
            if m is None:
                # cheap pre-load: manifest.json is small, and reading it
                # here keeps the topology-cache key stable across
                # processes instead of missing once per process
                mp = Path(self.anatomy_dir) / "manifest.json"
                if mp.is_file():
                    m = self._anatomy_manifest = json.loads(
                        mp.read_text())
                    self._manifest_hash = hashlib.sha256(
                        mp.read_bytes()).hexdigest()
                else:
                    m = {}
            return {"dataset_id": m.get("dataset_kind") or "anatomical",
                    "version": m.get("store_version"),
                    "manifest_hash": self._manifest_hash,
                    "region_mode": self.region_mode}
        if self.data_dir:
            return {"dataset_id": "flywire-v783-shiu-lif",
                    "version": "2025_783",
                    "manifest_hash": self._manifest_hash,
                    "region_mode": self.region_mode}
        # v1: the base graph is sampled from fba.base_seed, shared by every
        # individual (v0 sampled it per genome seed and is not reproducible
        # by this code path — a v0 recording must be replayed with M0 code).
        if self.synthetic_edges:
            version = (f"v1-n{self.synthetic_neurons}-e{self.synthetic_edges}"
                       f"-s{self.base_seed}")
        else:
            version = (f"v1-n{self.synthetic_neurons}-p{self.connectivity}"
                       f"-s{self.base_seed}")
        return {"dataset_id": "synthetic-fba", "version": version,
                "manifest_hash": None,
                "region_mode": self.region_mode}

    # ------------------------------------------------------------ init
    def initialize(self, phenotype: dict, batch_size: int, seed: int,
                   device: str, replicate_seeds: list[int] | None = None,
                   timer: PhaseTimer | None = None) -> None:
        timer = timer or PhaseTimer(enabled=False)
        self.batch_size = int(batch_size)
        self.seed = int(seed)
        self.device = torch.device(device if device else "cpu")
        self.phenotype = phenotype or {}
        # per-genome generator: artificial wiring only
        self._build_gen = torch.Generator(device="cpu").manual_seed(self.seed)
        if replicate_seeds is None:
            replicate_seeds = _default_replicate_seeds(self.seed, self.batch_size)
        if len(replicate_seeds) != self.batch_size:
            raise ValueError("len(replicate_seeds) must equal batch_size")
        self.replicate_seeds = [int(s) for s in replicate_seeds]
        self._gens = [torch.Generator(device=self.device)
                      for _ in range(self.batch_size)]

        with timer.phase("mutation_resolve"):
            self.params = dict(PARAMS)
            self.params.update(self.phenotype.get("params") or {})
            # A2: compartment diffusion time constant (MODEL_INFERENCE;
            # only used when a voltage_coupling matrix was supplied)
            self.params.setdefault("tauCoup", 5.0)

        self._load_base(timer)

        n_extra = int(self.phenotype.get("n_extra_neurons", 0) or 0)
        self.n = self.n_base + n_extra
        self.n_extra = n_extra
        self._organ_ranges: list[tuple[str, int, int]] = []
        self._organ_internal_p: dict[str, float] = {}
        off = self.n_base
        for organ in self.phenotype.get("artificial_organs", []):
            self._organ_ranges.append((organ["organ_id"], off,
                                       off + int(organ["size"])))
            # AFC grafts may carry their own internal density; M-series
            # organs without the field keep the historical constant
            if organ.get("internal_p") is not None:
                self._organ_internal_p[organ["organ_id"]] = \
                    float(organ["internal_p"])
            off += int(organ["size"])
        if off != self.n:
            raise ValueError("n_extra_neurons != sum(organ sizes)")

        with timer.phase("topology_construction"):
            self.W_extra, self.nnz_extra = self._build_extra()
        self.n_edges_base = self.nnz_base
        self.n_edges_total = self.nnz_base + self.nnz_extra
        self.nnz = self.n_edges_total
        self._W_override = None

        with timer.phase("state_init"):
            self._input_rates = torch.zeros(self.n, device=self.device)
            self._drive_idx = None
            self._drive_p = None
            self._silence_idx = None
            # D = round(tDelay/dt), rounded half-up with a float-error
            # guard (1.8/0.1 == 18.000000000000004 in binary floating
            # point, and Python's round() is banker's rounding)
            self.steps_delay = max(1, int(math.floor(
                self.params["tDelay"] / self.params["dt"] + 0.5 + 1e-9)))
            if self.voltage_coupling is not None:
                r, c, val = self.voltage_coupling
                self._vcoup = torch.sparse_coo_tensor(
                    torch.stack([torch.as_tensor(r, dtype=torch.int64),
                                 torch.as_tensor(c, dtype=torch.int64)]),
                    torch.as_tensor(val, dtype=torch.float32),
                    (self.n, self.n)).coalesce().to(self.device)
            else:
                self._vcoup = None
            self.reset()
            if self.runtime_mode in ACTIVE_MODES:
                self._init_active()

    # ------------------------------------------------------------ base graph
    def _load_base(self, timer: PhaseTimer) -> None:
        """Build (or reuse) the resident FBA0 reference graph.

        Values are kept raw; ``self._base_scale`` is applied at
        propagation time so that a ``wScale`` mutation is a scalar
        multiply, not a 14M-edge rebuild.
        """
        cache_dir = (self.runs_dir / "cache") if self.runs_dir else None
        if self.base_override is not None:
            # A2: caller-built graph (e.g. compartment-expanded circuit).
            # A3: optional 5th element = per-edge reversal class
            # (+1 excitatory / -1 inhibitory) for conductance mode.
            ov = self.base_override
            n_base, post, pre, w = ov[:4]
            self._base_esyn = (torch.as_tensor(ov[4], dtype=torch.float32)
                               if len(ov) > 4 else None)
            # raw (uncoalesced) edge arrays — split edges can share a
            # runtime (post,pre) node pair; esyn classes must be split
            # BEFORE coalescing, so keep the raw arrays (§19)
            self._base_raw = (torch.as_tensor(post, dtype=torch.int64),
                              torch.as_tensor(pre, dtype=torch.int64),
                              torch.as_tensor(w, dtype=torch.float32)) \
                if self._base_esyn is not None else None
            W = EventGraph.from_coo(
                torch.as_tensor(post, dtype=torch.int64),
                torch.as_tensor(pre, dtype=torch.int64),
                torch.as_tensor(w, dtype=torch.float32),
                int(n_base), int(n_base))
            self.W_base, self.n_base = W.to(self.device), int(n_base)
            self.nnz_base = self.W_base.nnz
            # override weights are final values — the caller has already
            # folded any scaling into them (anatomy-style semantics)
            self._base_scale_is_wscale = False
            self._base_scale = 1.0
            self._manifest_hash = None
            self.base_topology_key = f"override:{W.nnz}:{n_base}"
            self.region_mode = self._region_mode_arg
            return
        if self.anatomy_dir:
            # canonical anatomy store: region_mode stays whatever the
            # config asked for — anatomical ports are resolved through
            # the store, not through the synthetic pseudo-regions
            self.region_mode = self._region_mode_arg
        elif self.data_dir:
            self.region_mode = self._region_mode_arg
        elif self.synthetic:
            self.region_mode = self._region_mode_arg or SYNTHETIC_REGION_MODE
        else:
            raise BackendUnavailable(
                "no fba.anatomy_dir / fba.data_dir configured and "
                "synthetic=False")

        # dataset_identity() needs the manifest hash, which only exists
        # after the parquet has been read once; the first real-data build
        # therefore always misses the cache (by design: once per process).
        ident = self.dataset_identity()
        key = base_topology_key(ident, self.synthetic_neurons
                                if not self.data_dir else -1,
                                self.base_seed, self.connectivity,
                                self.synthetic_edges, str(self.device))
        hit = BASE_CACHE.get(key) if self.use_topology_cache else None
        if hit is not None:
            (self.W_base, self.n_base, self.nnz_base, self._manifest_hash,
             self._base_scale_is_wscale) = hit
            self.base_topology_key = key
            self._base_scale = (float(self.params["wScale"])
                                if self._base_scale_is_wscale else 1.0)
            return

        with timer.phase("topology_construction"):
            if self.anatomy_dir:
                (n_base, post, pre, w, manifest,
                 self._anatomy_manifest) = _load_anatomy(
                    Path(self.anatomy_dir), cache_dir)
                self._manifest_hash = manifest
                scale_is_wscale = False
            elif self.data_dir:
                n_base, post, pre, w, manifest = _load_connectome(
                    Path(self.data_dir), cache_dir)
                self._manifest_hash = manifest
                scale_is_wscale = False
            else:
                base_gen = torch.Generator(device="cpu").manual_seed(
                    self.base_seed)
                n_base = self.synthetic_neurons
                post, pre = _sample_edges(0, n_base, 0, n_base,
                                          self.connectivity, base_gen,
                                          n_edges=self.synthetic_edges)
                w = torch.rand(post.numel(), generator=base_gen)
                scale_is_wscale = True
            W = EventGraph.from_coo(post, pre, w, n_base, n_base)
            nnz = W.nnz
        with timer.phase("host_to_device"):
            W = W.to(self.device)

        self.W_base, self.n_base, self.nnz_base = W, n_base, nnz
        self._base_scale_is_wscale = scale_is_wscale
        self._base_scale = (float(self.params["wScale"]) if scale_is_wscale
                            else 1.0)
        # rebuild the key: for real data the manifest hash is only known now
        key = base_topology_key(self.dataset_identity(),
                                n_base if not self.data_dir else -1,
                                self.base_seed, self.connectivity,
                                self.synthetic_edges, str(self.device))
        self.base_topology_key = key
        if self.use_topology_cache:
            BASE_CACHE.put(key, (W, n_base, nnz, self._manifest_hash,
                                 scale_is_wscale))

    # ------------------------------------------------------------ inspection
    @property
    def W(self):
        """The full ``N x N`` connectivity as one CSR tensor, with
        ``wScale`` already applied — materialised on demand for
        inspection, tests and replay diffing. The simulation itself never
        builds it (see ``propagate``)."""
        if getattr(self, "_W_override", None) is not None:
            return self._W_override
        base = self.W_base.to_coo(size=(self.n, self.n),
                                  scale=self._base_scale).coalesce()
        if self.W_extra is None:
            return base.to_sparse_csr()
        ex = self.W_extra.to_coo().coalesce()
        idx = torch.cat([base.indices(), ex.indices()], dim=1)
        vals = torch.cat([base.values(), ex.values()])
        return torch.sparse_coo_tensor(idx, vals, (self.n, self.n)) \
            .coalesce().to_sparse_csr()

    @W.setter
    def W(self, value):
        """Install an explicit matrix (tests / fixed micro-networks);
        ``propagate`` then uses it instead of the base+extra split."""
        self._W_override = value

    # ------------------------------------------------- A3 active mode
    def _init_active(self):
        """Active membrane state for ``physiology.active_idx`` nodes
        (mixed fidelity §25: others keep the LIF path untouched).

        phenotype["physiology"] keys:
          active_idx        runtime node indices that run HH dynamics
          membrane          {Cm, g_leak, E_leak, V_rest} per active node
          channels          {name: {e_rev, g_bar: [per-node]}}
          synapse           {E_exc, E_inh, tau_syn, weight_to_g,
                             model, unknown_nt_mode}
          integration       {method, dt}
          v_spike           rising-edge spike detect (mV)
          stability         {v_min, v_max}
        """
        from ..physio.channels import build_channel
        ph = self.phenotype.get("physiology") or {}
        idx = ph.get("active_idx")
        if not idx:
            raise ValueError("active_hh_v0 requires "
                             "physiology.active_idx")
        d, B = self.device, self.batch_size
        self._active_idx = torch.as_tensor(idx, dtype=torch.long,
                                           device=d)
        na = len(idx)

        def nodepar(name, default):
            vals = (ph.get("membrane") or {}).get(name)
            if vals is None:
                return torch.full((na,), float(default), device=d)
            t = torch.as_tensor(vals, dtype=torch.float32, device=d)
            return t.expand(na).clone() if t.numel() == 1 else t

        self._ph = {k: nodepar(k, dv) for k, dv in
                    (("Cm", 1.0), ("g_leak", 0.3), ("E_leak", -54.4),
                     ("V_rest", -60.0))}
        # channels: pluggable models, per-node density
        self._ch_models, self._ch_gbar, self._ch_state = [], [], []
        for name, spec in (ph.get("channels") or {}).items():
            model = build_channel(spec)
            gbar = torch.as_tensor(spec.get("g_bar", 1.0),
                                   dtype=torch.float32, device=d)
            gbar = gbar.expand(na).clone() if gbar.numel() == 1 else gbar
            self._ch_models.append(model)
            self._ch_gbar.append(gbar)
        syn = ph.get("synapse") or {}
        self._syn = {"E_exc": float(syn.get("E_exc", 0.0)),
                     "E_inh": float(syn.get("E_inh", -80.0)),
                     "tau_syn": float(syn.get("tau_syn",
                                              self.params["tauSyn"])),
                     "weight_to_g": float(syn.get("weight_to_g", 0.02)),
                     "model": syn.get("model", "conductance_v0"),
                     # A3.1 §31: the receptor model emits
                     # unknown_sign_mode; keep the A3 unknown_nt_mode
                     # key as an alias for older overlays
                     "unknown_nt_mode": syn.get(
                         "unknown_sign_mode",
                         syn.get("unknown_nt_mode", "neutral"))}
        self._v_spike = float(ph.get("v_spike", -20.0))
        stab = ph.get("stability") or {}
        self._v_min = float(stab.get("v_min", -110.0))
        self._v_max = float(stab.get("v_max", 90.0))
        self._drive_g = float((ph.get("drive") or {})
                              .get("g_exc", 5.0))
        # split base graph by edge reversal class (§19-21). Uses the
        # raw pre-coalescing arrays: multiple split edges may share a
        # runtime (post,pre) pair; each class graph coalesces itself.
        self._W_exc = None
        self._W_inh = None
        if self._base_esyn is not None:
            ip, ir, iv = self._base_raw
            inh = (self._base_esyn < -30.0).to(ip.device)
            ip, ir, iv = ip.to(d), ir.to(d), iv.to(d)
            if inh.any():
                self._W_inh = EventGraph.from_coo(
                    ip[inh], ir[inh], iv[inh], self.n_base,
                    self.n_base).to(d)
            m = ~inh
            self._W_exc = EventGraph.from_coo(
                ip[m], ir[m], iv[m], self.n_base,
                self.n_base).to(d)
        else:
            self._W_exc = self.W_base
        # per-active-node axial self-conductance g_ax,i = -c_ii (the
        # coupling matrix's diagonal) for the semi-implicit V update
        if self._vcoup is not None:
            coo = self._vcoup.coalesce()
            ii, jj, vv = coo.indices()[0], coo.indices()[1], coo.values()
            dm = ii == jj
            diag = torch.zeros(self.n, device=d)
            diag[ii[dm]] = vv[dm]
            self._ax_diag = (-diag[self._active_idx]).clamp(min=0.0)
        else:
            self._ax_diag = torch.zeros(na, device=d)
        self._amask = torch.zeros(self.n, dtype=torch.bool, device=d)
        self._amask[self._active_idx] = True
        # conductance state, channel gates, resting voltage (also the
        # reset() re-entry path)
        self._reset_active()

    def _reset_active(self) -> None:
        """Re-initialise conductance/channel state (reset() re-entry:
        v/g/delay_buf are rebuilt by reset() itself)."""
        B, d = self.batch_size, self.device
        na = int(self._active_idx.numel())
        self.g_exc = torch.zeros((B, self.n), device=d)
        self.g_inh = torch.zeros((B, self.n), device=d)
        self.delay_buf_inh = torch.zeros_like(self.delay_buf)
        self._i_ext = torch.zeros((B, self.n), device=d)
        vrest = self._ph["V_rest"]
        self.v[:, self._active_idx] = vrest[None, :]
        V0 = vrest.expand(B, na)
        self._ch_state = [m.initial_state(V0) for m in self._ch_models]
        self._v_prev = self.v.clone()
        self._num_failure = None
        self._run_ct = torch.zeros((B, na), dtype=torch.long, device=d)

    def set_current_injection(self, currents: dict) -> None:
        """Constant current-density injection per runtime node
        (µA/cm²), active nodes only — used by the A3.1 calibration /
        single-cell benchmark suite (§11 I_external)."""
        if self._active_idx is None:
            raise RuntimeError("set_current_injection requires an "
                             "active runtime mode")
        for idx, cur in currents.items():
            self._i_ext[:, int(idx)] = float(cur)

    # ------------------------------------------------- trace (§41)
    def enable_trace(self, node_indices, dt_sample: float = 0.1):
        """Record V + channel state for a few nodes each step (single-
        cell benchmark artifact; keep the node list tiny)."""
        self._trace_nodes = [int(i) for i in node_indices]
        self._trace = {i: {"t": [], "v": [], "g_exc": [], "g_inh": [],
                           "ch": {}} for i in self._trace_nodes}
        self._trace_dt = float(dt_sample)

    def _trace_step(self):
        if not getattr(self, "_trace_nodes", None):
            return
        pos = {int(j): k for k, j in enumerate(self._active_idx.tolist())}
        for i in self._trace_nodes:
            a = pos.get(i)
            tr = self._trace[i]
            tr["t"].append(float(self.t_ms))
            tr["v"].append(float(self.v[0, i].item()))
            tr["g_exc"].append(float(self.g_exc[0, i].item()))
            tr["g_inh"].append(float(self.g_inh[0, i].item()))
            if a is not None:
                for name, st in zip(self._ch_models, self._ch_state):
                    tr["ch"].setdefault(name.name, []).append(
                        [float(x) for x in st[0, a].tolist()])

    def get_trace(self) -> dict:
        return self._trace

    # ------------------------------------------------------------ regions
    def region_range(self, region: str) -> tuple[int, int]:
        """Neuron index range for ``fba0:<region>``. Only defined under
        synthetic-region-v0 (equal contiguous slices of FBA0)."""
        if self.region_mode != SYNTHETIC_REGION_MODE:
            raise UnsupportedAttachmentRegion(
                f"fba0:{region}: no FBA0 region->neuron mapping available "
                f"(region_mode={self.region_mode!r}); refusing to wire "
                "randomly")
        if region not in SYNTHETIC_REGIONS:
            raise UnsupportedAttachmentRegion(
                f"fba0:{region}: unknown synthetic region")
        k = SYNTHETIC_REGIONS.index(region)
        per = self.n_base // len(SYNTHETIC_REGIONS)
        lo = k * per
        hi = self.n_base if k == len(SYNTHETIC_REGIONS) - 1 else lo + per
        return lo, hi

    def _endpoint_range(self, name: str) -> tuple[int, int]:
        if name.startswith("fba0:"):
            return self.region_range(name.split(":", 1)[1])
        if name == "fba0":
            return 0, self.n_base
        rng = next((r for r in self._organ_ranges if r[0] == name), None)
        if rng is None:
            raise ValueError(f"attachment endpoint {name!r}: unknown organ")
        return rng[1], rng[2]

    def _endpoint_sel(self, att: dict, which: str):
        """Resolve one attachment side to either an (lo, hi) contiguous
        range or an explicit int64 index tensor (AFC graft wiring)."""
        idx = att.get(f"{which}_idx")
        if idx is not None:
            return torch.as_tensor(idx, dtype=torch.int64)
        return self._endpoint_range(att[which])

    def _sample_extra(self):
        """Sample the artificial index set: organ-internal edges
        (p=ORGAN_INTERNAL_P inside each organ block) and attachment edges
        (source (pre) -> target (post) pairs at p=ATTACHMENT_P;
        'bidirectional' also adds target -> source).

        Returns ``(post, pre, raw, group)`` where ``group`` is 0 for
        organ-internal edges and ``1 + attachment index`` otherwise, so
        the per-edge weight scale can be applied afterwards without
        re-sampling. Endpoints are resolved by name; nothing is wired at
        random across the whole network.
        """
        gen = self._build_gen
        posts, pres, raws, groups = [], [], [], []

        def add(p_, r_, group):
            posts.append(p_)
            pres.append(r_)
            raws.append(torch.rand(p_.numel(), generator=gen))
            groups.append(torch.full((p_.numel(),), group, dtype=torch.int64))

        for _oid, lo, hi in self._organ_ranges:
            add(*_sample_edges(lo, hi, lo, hi,
                               self._organ_internal_p.get(
                                   _oid, ORGAN_INTERNAL_P), gen), 0)
        for i, att in enumerate(self.phenotype.get("attachments", [])):
            # AFC: an endpoint is either a named contiguous range
            # (historical) or an explicit ``*_idx`` index list carrying
            # wiring resolved against an anatomical store by the graft
            # compiler — the backend itself stays dataset-agnostic.
            s = self._endpoint_sel(att, "source")
            t = self._endpoint_sel(att, "target")
            p = float(att.get("p", ATTACHMENT_P))
            legs = [(t, s)]
            if att.get("direction", "forward") == "bidirectional":
                legs.append((s, t))
            for post_sel, pre_sel in legs:
                add(*_sample_mixed(post_sel, pre_sel, p, gen), i + 1)
        if not posts:
            e = torch.empty(0, dtype=torch.int64)
            return e, e.clone(), torch.empty(0), e.clone()
        return (torch.cat(posts), torch.cat(pres), torch.cat(raws),
                torch.cat(groups))

    def _build_extra(self):
        """Build the per-genome artificial sparse matrix (``None`` when the
        genome has no organs). The sampled index set is cached by
        ``organ_topology_hash``, so a parameter-only child reuses its
        parent's wiring instead of re-drawing it."""
        if not self._organ_ranges and not self.phenotype.get("attachments"):
            return None, 0
        key = organ_topology_hash(self.phenotype, self.seed)
        hit = ORGAN_CACHE.get(key) if self.use_topology_cache else None
        if hit is None:
            hit = self._sample_extra()
            if self.use_topology_cache:
                ORGAN_CACHE.put(key, hit)
        post, pre, raw, group = hit
        self.organ_topology_key = key
        if post.numel() == 0:
            return None, 0
        wscale = float(self.params["wScale"])
        scales = torch.tensor(
            [wscale] + [wscale * float(a.get("weight_scale", 1.0))
                        for a in self.phenotype.get("attachments", [])],
            dtype=raw.dtype)
        vals = raw * scales[group]
        W = EventGraph.from_coo(post, pre, vals, self.n, self.n)
        return W.to(self.device), W.nnz

    # ------------------------------------------------------------ state
    def reset(self) -> None:
        p = self.params
        d, B, n = self.device, self.batch_size, self.n
        self.t_ms = 0.0
        self.v = torch.full((B, n), p["v0"], device=d)
        self.g = torch.zeros((B, n), device=d)
        # D + 1 slots: a value written at step t into slot (ptr-1) mod L
        # comes back round exactly D steps later (see _one_step)
        self.delay_buf = torch.zeros((B, self.steps_delay + 1, n), device=d)
        self._delay_ptr = 0
        self.refrac = torch.full((B, n), p["tRefrac"], device=d)
        self.spikes = torch.zeros((B, n), device=d)
        self.spike_counts = torch.zeros((B, n), dtype=torch.long, device=d)
        for g, s in zip(self._gens, self.replicate_seeds):
            g.manual_seed(s)
        if self._active_idx is not None:
            self._reset_active()
        self._tel_reset()
        if self._trace_nodes:
            self._trace = {i: {"t": [], "v": [], "g_exc": [],
                               "g_inh": [], "ch": {}}
                           for i in self._trace_nodes}

    # ------------------------------------------------- telemetry (§19)
    def set_telemetry(self, node_indices):
        """Enable per-node telemetry on a small set of runtime nodes
        (G0.1 §19): counts artificial-synapse *events* arriving at each
        node (from W_extra), accumulates synaptic input, and tracks
        membrane voltage mean/peak per step. Events are counted at
        emission — deterministic, arrival is D steps later. Only
        W_extra edges are measured: base-edge contributions are
        identical across paired conditions and cancel in deltas."""
        self._tel_nodes = [int(i) for i in node_indices]
        self._tel_reset()
        if self.W_extra is None or not self._tel_nodes:
            return
        W = self.W_extra.as_csr().to_sparse_coo().coalesce()
        post, pre = W.indices()
        lut = {nd: k for k, nd in enumerate(self._tel_nodes)}
        mask = torch.tensor([int(p) in lut for p in post.tolist()])
        tel_w = torch.zeros((len(self._tel_nodes), self.n),
                            device=self.device)
        if mask.any():
            rows = torch.tensor([lut[int(p)] for p in post[mask]],
                                dtype=torch.long)
            tel_w[rows, pre[mask]] += W.values()[mask]
        self._tel_W = tel_w
        self._tel_adj = (tel_w != 0).to(self.v.dtype)

    def _tel_reset(self):
        B = getattr(self, "batch_size", 1)
        n_tel = len(getattr(self, "_tel_nodes", []) or [])
        self._tel_W = None
        self._tel_adj = None
        d = self.device
        self._tel_events = torch.zeros((B, n_tel), device=d)
        self._tel_syn = torch.zeros((B, n_tel), device=d)
        self._tel_vsum = torch.zeros((B, n_tel), device=d)
        self._tel_vpeak = torch.full((B, n_tel), -1e9, device=d)
        self._tel_steps = 0

    def get_telemetry(self) -> dict | None:
        if not getattr(self, "_tel_nodes", None):
            return None
        steps = max(1, self._tel_steps)
        out = []
        for k, nd in enumerate(self._tel_nodes):
            out.append({
                "runtime_idx": nd,
                "input_event_count":
                    self._tel_events[:, k].tolist(),
                "synaptic_input_sum":
                    self._tel_syn[:, k].tolist(),
                "membrane_voltage_mean":
                    (self._tel_vsum[:, k] / steps).tolist(),
                "membrane_voltage_peak":
                    self._tel_vpeak[:, k].tolist(),
            })
        return {"nodes": out, "scope": "W_extra artificial edges only"}

    def set_inputs(self, drive: dict) -> None:
        """Install a drive. Only the driven neurons are kept as an index
        list: the per-step Poisson draw costs one uniform per *driven*
        neuron, not one per neuron (M1 §2)."""
        rates = torch.zeros(self.n, device=self.device)
        for key, hz in (drive.get("rates_hz") or {}).items():
            if isinstance(key, str) and key == "all":
                rates[:] = hz
            elif isinstance(key, str) and key.startswith("slice:"):
                lo, hi = key.split(":")[1].split("-")
                rates[int(lo):int(hi)] = hz
            else:
                rates[int(key)] = hz
        self._input_rates = rates
        idx = torch.nonzero(rates, as_tuple=False).flatten()
        if idx.numel():
            self._drive_idx = idx
            self._drive_p = rates[idx] * self.params["dt"] / 1000.0
        else:
            self._drive_idx = None
            self._drive_p = None
        sil = [int(i) for i in (drive.get("silence") or [])]
        self._silence_idx = (torch.tensor(sil, dtype=torch.int64,
                                          device=self.device)
                             if sil else None)

    def force_spikes(self, neuron_ids: list[int] | None) -> None:
        """Debug/test hook: force these neurons to spike every step."""
        if not neuron_ids:
            self._force = None
            return
        self._force = torch.tensor([int(i) for i in neuron_ids],
                                   dtype=torch.int64, device=self.device)

    def propagate(self, spikes: torch.Tensor) -> torch.Tensor:
        """Synaptic input to every post neuron given a (B, N) spike
        matrix: I[b, post] = sum_pre W[post, pre] * spikes[b, pre].

        Event-driven (see fba/eventgraph.py): only the edges leaving
        neurons that actually spiked are touched. Base and artificial
        graphs accumulate into the same output; the base is resident and
        its values are raw, so ``wScale`` enters as a scalar.
        """
        if getattr(self, "_W_override", None) is not None:
            return torch.sparse.mm(self._W_override, spikes.T).T
        t0 = time.perf_counter()
        out = torch.zeros((spikes.shape[0], self.n), device=spikes.device,
                          dtype=spikes.dtype)
        out_inh = None
        if self._W_exc is not None or self._W_inh is not None:
            # active mode: the base graph is split by reversal class —
            # excitatory and inhibitory arrive in separate buckets (§19)
            events, edges = 0, 0
            if self._W_exc is not None:
                events, edges = self._W_exc.propagate(
                    spikes, out, scale=self._base_scale,
                    col_limit=self.n_base, dense_above=self._dense_above)
            if self._W_inh is not None:
                out_inh = torch.zeros_like(out)
                e2, d2 = self._W_inh.propagate(
                    spikes, out_inh, scale=self._base_scale,
                    col_limit=self.n_base, dense_above=self._dense_above)
                events, edges = max(events, e2), edges + d2
        else:
            events, edges = self.W_base.propagate(
                spikes, out, scale=self._base_scale,
                col_limit=self.n_base, dense_above=self._dense_above)
        if self.W_extra is not None:
            e2, d2 = self.W_extra.propagate(spikes, out,
                                            dense_above=self._dense_above)
            events, edges = max(events, e2), edges + d2
            self._act_edges_extra += d2
            if self._tel_adj is not None:
                # telemetry (§19): count artificial-edge events and
                # synaptic input arriving at the watched nodes
                self._tel_events += self._tel_adj.mm(spikes.T).T
                self._tel_syn += self._tel_W.mm(spikes.T).T
        self._act_steps += 1
        self._act_lane_steps += int(spikes.shape[0])
        self._act_events += events
        self._act_edges += edges
        self._act_wall += time.perf_counter() - t0
        return (out, out_inh) if out_inh is not None \
            or self._W_exc is not None else out

    # ------------------------------------------------------------ step
    def _one_step(self) -> None:
        p, d = self.params, self.device
        dt = p["dt"]
        L = self.delay_buf.shape[1]
        ptr = self._delay_ptr
        # arrivals scheduled D steps ago
        delayed = self.delay_buf[:, ptr, :]
        act = (self.refrac >= p["tRefrac"]).float()
        if self._active_idx is not None:
            # ---- A3 active path: conductance arrivals split exc/inh,
            # explicit membrane equation on active nodes, LIF elsewhere
            aidx = self._active_idx
            amask = self._amask
            delayed_inh = self.delay_buf_inh[:, ptr, :]
            # arrivals stay in weight units; weight_to_g converts them
            # to conductance density for the membrane equation (§19).
            # The passive g bucket keeps raw weight semantics.
            w2g = self._syn["weight_to_g"]
            self.g_exc = self.g_exc * (1 - dt / self._syn["tau_syn"]) \
                + delayed * act * w2g
            self.g_inh = self.g_inh * (1 - dt / self._syn["tau_syn"]) \
                + delayed_inh * act * w2g
            # passive arrivals bucket for LIF nodes keeps old semantics
            self.g = self.g * (1 - dt / p["tauSyn"]) \
                + (delayed + delayed_inh) * act
            v_next = self.v + (dt / p["tauMem"]) \
                * (self.g - (self.v - p["vRest"]))
            if self._vcoup is not None:
                axial = torch.sparse.mm(self._vcoup, self.v.T).T
                v_next = v_next + (dt / p["tauCoup"]) * axial * (
                    ~amask)[None, :]
            else:
                axial = None
            self._v_prev = self.v
            # membrane equation on active nodes (§11):
            # Cm dV/dt = -I_leak - I_channels - I_syn + I_axial + I_ext
            # integrated semi-implicitly (exponential Euler in V with
            # gates frozen at V(t); gates themselves cnexp, §22):
            #   V(t+dt) = V_inf + (V - V_inf) * exp(-b dt / Cm)
            # with b = total conductance density, a = drive sum.
            Va = self.v[:, aidx]
            g_exc_a = self.g_exc[:, aidx]
            g_inh_a = self.g_inh[:, aidx]
            ax = (axial[:, aidx] if axial is not None
                  else torch.zeros_like(Va))
            ax_off = ax + self._ax_diag[None, :] * Va
            b_cond = (self._ph["g_leak"][None, :] + g_exc_a + g_inh_a
                      + self._ax_diag[None, :])
            a_cur = (self._ph["g_leak"][None, :]
                     * self._ph["E_leak"][None, :]
                     + g_exc_a * self._syn["E_exc"]
                     + g_inh_a * self._syn["E_inh"] + ax_off)
            if self._i_ext is not None:
                # A3.1 §11: +I_external (current-density injection,
                # used by the calibration/benchmark suite)
                a_cur = a_cur + self._i_ext[:, aidx]
            bad_ch = False
            for i, (ch, gbar) in enumerate(
                    zip(self._ch_models, self._ch_gbar)):
                st = self._ch_state[i]
                ge = gbar[None, :] * ch.conductance(Va, st)
                i_ch = ge * (Va - ch.e_rev)
                # §55: unphysical channel current — non-finite or an
                # absurd magnitude (|I| > 1e6 µA/cm²) is a failure,
                # never silently clipped
                if bool((~torch.isfinite(i_ch)).any()) or bool(
                        (i_ch.abs() > 1e6).any()):
                    bad_ch = True
                b_cond = b_cond + ge
                a_cur = a_cur + ge * ch.e_rev
                self._ch_state[i] = ch.advance(dt, Va, st)
            b_cond = b_cond.clamp(min=1e-9)
            v_inf = a_cur / b_cond
            Va_new = v_inf + (Va - v_inf) * torch.exp(
                -b_cond * dt / self._ph["Cm"][None, :])
            # §23-24: numerical guards — never silently clip
            bad = ~torch.isfinite(Va_new) | (Va_new < self._v_min) \
                | (Va_new > self._v_max)
            # §55: gates are probabilities — flag non-finite AND
            # out-of-[0,1] state (small tolerance for float noise)
            bad_g = any((~torch.isfinite(s).all())
                        or bool(((s < -1e-4) | (s > 1.0 + 1e-4)).any())
                        for s in self._ch_state)
            # §55 AIS/active runaway: an active node spiking every step
            # for >3 ms straight is a ≥10 kHz non-physical rate
            if not hasattr(self, "_run_ct") or self._run_ct is None \
                    or self._run_ct.shape[0] != self.batch_size:
                self._run_ct = torch.zeros(
                    (self.batch_size, aidx.numel()),
                    dtype=torch.long, device=d)
            just = (self._v_prev[:, aidx] < self._v_spike) \
                & (self.v[:, aidx] >= self._v_spike)
            self._run_ct = torch.where(just, self._run_ct + 1,
                                       torch.zeros_like(self._run_ct))
            runaway = int((self._run_ct * dt > 3.0).sum())
            if bad.any() or bad_g or bad_ch or runaway:
                reasons = []
                if bad.any():
                    reasons.append("voltage out of range or non-finite")
                if bad_g:
                    reasons.append("gate state non-finite/outside [0,1]")
                if bad_ch:
                    reasons.append("unphysical channel current")
                if runaway:
                    reasons.append(f"active/AIS runaway x{runaway}")
                self._num_failure = {
                    "status": "NUMERICAL_FAILURE",
                    "reason": "; ".join(reasons),
                    "at_t_ms": round(float(self.t_ms), 4),
                    "n_bad": int(bad.sum())}
            else:
                v_next[:, aidx] = Va_new
            self.v = v_next
        else:
            self.g = self.g * (1 - dt / p["tauSyn"]) + delayed * act
            self.v = self.v + (dt / p["tauMem"]) \
                * (self.g - (self.v - p["vRest"]))
            if self._vcoup is not None:
                # passive diffusion between an entity's compartment
                # nodes (A2): dv_i = dt/tauCoup * Σ_j c_ij (v_j − v_i)
                self.v = self.v + (dt / p["tauCoup"]) \
                    * torch.sparse.mm(self._vcoup, self.v.T).T
        if getattr(self, "_tel_nodes", None):
            # compartment voltage telemetry, sampled post-coupling /
            # pre-reset so dendritic peaks aren't clipped away (§19)
            vt = self.v[:, self._tel_nodes]
            self._tel_vsum += vt
            self._tel_vpeak = torch.maximum(self._tel_vpeak, vt)
            self._tel_steps += 1
        if self._drive_idx is not None:
            u = torch.stack([torch.rand((self._drive_idx.numel(),), device=d,
                                        generator=g) for g in self._gens])
            # stimulus weight is wScale * scalePoisson (0.275*250 = 68.75
            # mV), matching the reference PoissonInput weight w_syn*f_poi.
            # Semantics <= 2 applied scalePoisson alone as a raw 250 mV
            # kick — a porting slip, fixed under semantics version 3.
            stim = (u < self._drive_p[None, :]).to(self.v.dtype) \
                * (p["wScale"] * p["scalePoisson"])
            if self._active_idx is not None:
                # driven ACTIVE nodes get a conductance pulse into
                # g_exc (synaptic bombardment), passive nodes keep the
                # legacy voltage kick
                stim_a = (u < self._drive_p[None, :]).to(self.v.dtype) \
                    * self._drive_g
                am = self._amask[self._drive_idx][None, :]
                self.g_exc[:, self._drive_idx] += stim_a * am
                self.v[:, self._drive_idx] += stim * (~am)
            else:
                self.v[:, self._drive_idx] += stim
        if self._active_idx is not None:
            spikes = (
                ((self.v >= p["vThr"]) & (self.refrac >= p["tRefrac"])
                 & (~self._amask)[None, :])
                | ((self.v >= self._v_spike)
                   & (self._v_prev < self._v_spike)
                   & (self.refrac >= p["tRefrac"])
                   & self._amask[None, :])).float()
        else:
            spikes = ((self.v >= p["vThr"])
                      & (self.refrac >= p["tRefrac"])).float()
        if self._force is not None:
            spikes[:, self._force] = 1.0
        if self._silence_idx is not None:
            spikes[:, self._silence_idx] = 0.0
        if self._active_idx is not None:
            # LIF instant reset only on passive nodes; active nodes
            # repolarize through their channels (§12)
            rst = (spikes > 0) & (~self._amask)[None, :]
            self.v = torch.where(
                rst, torch.full_like(self.v, p["vReset"]), self.v)
            self.g = self.g * (1 - spikes * (~self._amask)[None, :])
        else:
            self.v = torch.where(
                spikes > 0, torch.full_like(self.v, p["vReset"]), self.v)
            self.g = self.g * (1 - spikes)
        self.refrac = torch.where(spikes > 0, torch.zeros_like(self.refrac),
                                  self.refrac + dt)
        self.spikes = spikes
        self.spike_counts += spikes.long()
        self._trace_step()
        # schedule this step's spikes to arrive in D steps: slot (ptr-1)
        # mod L was last read one step ago, so nothing unread is lost
        prop = self.propagate(spikes)
        if isinstance(prop, tuple):
            self.delay_buf[:, (ptr - 1) % L, :] = prop[0]
            self.delay_buf_inh[:, (ptr - 1) % L, :] = prop[1] \
                if prop[1] is not None else 0.0
        else:
            self.delay_buf[:, (ptr - 1) % L, :] = prop
        self._delay_ptr = (ptr + 1) % L
        self.t_ms += dt

    def step(self, n_steps: int = 1) -> None:
        with torch.no_grad():
            for _ in range(int(n_steps)):
                if self._guard is not None or self._num_failure is not None:
                    break
                self._one_step()
                if self._run_limits:
                    self._check_run_limits()

    # ------------------------------------------------------------ run guard
    def set_run_limits(self, limits: dict | None) -> None:
        """Deterministic runaway guard (M1.5 §8).

        ``max_propagated_edge_events`` (the preferred limit), and
        ``max_spike_events`` are pure functions of the simulation — the
        same genome under the same seeds trips at the same step on any
        machine. ``max_wall_seconds`` is an operational backstop only:
        wall time depends on the host, so it is reported as
        ``deterministic: false`` and must never be the sole scientific
        criterion. All limits default to unset — an empty config behaves
        exactly like M1.
        """
        self._run_limits = dict(limits or {})
        self._guard = None
        self._guard_wall_start = None

    def _check_run_limits(self) -> None:
        lim = self._run_limits
        reason = None
        edge_lim = int(lim.get("max_propagated_edge_events") or 0)
        if edge_lim and self._act_edges >= edge_lim:
            reason = "max_propagated_edge_events"
        spike_lim = int(lim.get("max_spike_events") or 0)
        if reason is None and spike_lim and self._act_events >= spike_lim:
            reason = "max_spike_events"
        rate_lim = float(lim.get("max_mean_rate_hz") or 0.0)
        if reason is None and rate_lim and self.t_ms > 0:
            rate = float(self.spike_counts.float().mean().item()) \
                / (self.t_ms / 1000.0)
            if rate >= rate_lim:
                reason = "max_mean_rate_hz"
        wall_lim = float(lim.get("max_wall_seconds") or 0.0)
        if reason is None and wall_lim \
                and self._guard_wall_start is not None \
                and time.perf_counter() - self._guard_wall_start >= wall_lim:
            reason = "max_wall_seconds"
        if reason is not None:
            self._guard = {
                "tripped": True,
                "reason": reason,
                "at_t_ms": self.t_ms,
                "propagated_edge_events": int(self._act_edges),
                "spike_events": int(self._act_events),
                # deterministic = the trip depends only on the simulated
                # event stream, not on the host that happened to run it
                "deterministic": reason != "max_wall_seconds",
            }

    def run(self, duration_ms: float) -> dict:
        n = max(1, int(round(duration_ms / self.params["dt"])))
        t0 = time.perf_counter()
        t_ms_before = self.t_ms
        if self._guard is None and self._num_failure is None:
            if self._guard_wall_start is None \
                    and (self._run_limits or {}).get("max_wall_seconds"):
                self._guard_wall_start = t0
            self.step(n)
        wall = time.perf_counter() - t0
        out = {"simulated_ms": float(self.t_ms - t_ms_before),
               "wall_s": wall,
               "spikes_total": int(self.spike_counts.sum().item())}
        if self._guard is not None:
            out["guard"] = dict(self._guard)
        if self._num_failure is not None:
            out["numerical_failure"] = dict(self._num_failure)
        return out

    # ------------------------------------------------------------ report
    def _rates_hz(self):
        t_s = max(self.t_ms, 1e-9) / 1000.0
        return self.spike_counts.float() / t_s

    def _vram(self):
        if self.device.type == "cuda":
            return int(torch.cuda.memory_allocated(self.device))
        return None

    def vram_info(self) -> dict:
        """allocated / reserved / total bytes for this backend's device
        (all None on CPU)."""
        if self.device.type != "cuda":
            return {"allocated": None, "reserved": None, "total": None}
        return {"allocated": int(torch.cuda.memory_allocated(self.device)),
                "reserved": int(torch.cuda.memory_reserved(self.device)),
                "total": int(torch.cuda.get_device_properties(
                    self.device).total_memory)}

    def get_neuron_activity(self, neuron_indices: list[int], lane: int = 0):
        validate_neuron_selection(neuron_indices, lane, self.n, self.batch_size)
        indices = torch.tensor(neuron_indices, dtype=torch.long, device=self.device)
        counts = self.spike_counts[lane].index_select(0, indices)
        return {"t_ms": float(self.t_ms), "n_neurons": self.n,
                "n_base": self.n_base, "neuron_indices": list(neuron_indices),
                "spike_counts": counts.detach().cpu().tolist()}

    def get_state_summary(self) -> dict:
        rates = self._rates_hz()
        return {
            "t_ms": self.t_ms,
            "mean_rate_hz": float(rates.mean().item()),
            "active_fraction": float((self.spike_counts.sum(1) > 0)
                                     .float().mean().item()),
            "per_batch_spike_counts": self.spike_counts.sum(1).tolist(),
            "per_batch_mean_rate_hz": rates.mean(1).tolist(),
            "vram_bytes": self._vram(),
            "vram": self.vram_info(),
            "replicate_seeds": list(self.replicate_seeds),
            "n_neurons": self.n,
            "n_base": self.n_base,
            "n_artificial_neurons": self.n_extra,
            "n_artificial_organs": len(self._organ_ranges),
            "n_attachments": len(self.phenotype.get("attachments") or []),
            "nnz": self.nnz,
            "nnz_base": self.nnz_base,
            "nnz_artificial": self.nnz_extra,
            "region_mode": self.region_mode,
            "topology": {"base_key": getattr(self, "base_topology_key", None),
                         "organ_key": getattr(self, "organ_topology_key", None),
                         "base_cache": BASE_CACHE.stats(),
                         "organ_cache": ORGAN_CACHE.stats()},
            "semantics": self.semantics(),
            "activity": self.activity_stats(),
            "resource": self.marginal_resource_cost(),
            "guard": self._guard,
        }

    # ------------------------------------------------------------ activity
    def activity_stats(self) -> dict:
        """How much of the network the evaluation actually touched.

        This is what decides whether event-driven propagation is still
        the right path (M1 §2.4-4): the gather cost scales with
        ``active_edges_per_step``, the dense product with ``nnz``. A
        hyperactive mutant showing an ``active_edge_ratio`` near 1 is
        slowing its worker down and must be visible, not silently
        expensive.
        """
        lane_steps = max(1, self._act_lane_steps)
        events_per_step = self._act_events / lane_steps
        edges_per_step = self._act_edges / lane_steps
        return {
            "steps": self._act_steps,
            "lane_steps": self._act_lane_steps,
            "active_neurons_per_step": round(events_per_step, 4),
            "active_presynaptic_ratio": round(
                events_per_step / max(1, self.n), 8),
            "active_edges_per_step": round(edges_per_step, 4),
            "active_edge_ratio": round(
                edges_per_step / max(1, self.nnz), 8),
            "event_propagation_seconds": round(self._act_wall, 6),
            "propagation_backend": self.propagation_backend,
            "dense_above": (None if self._dense_above is None
                            else self._dense_above),
        }

    # ------------------------------------------------------------ resource
    def marginal_resource_cost(self) -> dict:
        """The *individual's own* cost, not the worker's.

        The 139k/14M FBA0 base is shared by every organism in the run, so
        charging an individual for it (worker RSS, total VRAM) would
        score the substrate, not the genome — and would make fitness
        depend on which GPU ran the job. Only what the genome adds is
        counted here; measured RSS/VRAM stay in telemetry (M1 §2.4-8).
        """
        f = 4                                   # float32
        lanes = max(1, getattr(self, "batch_size", 1))
        # per artificial neuron: v, g, refrac, spikes + (D+1) delay slots
        slots = int(getattr(self, "delay_buf", None).shape[1]) \
            if getattr(self, "delay_buf", None) is not None else 1
        per_neuron_bytes = f * (4 + slots) + 8  # +int64 spike_counts
        neuron_bytes = self.n_extra * per_neuron_bytes
        # per artificial edge: value (f32) + row index (int64)
        edge_bytes = self.nnz_extra * (f + 8)
        organ_bytes = {oid: (hi - lo) * per_neuron_bytes
                       for oid, lo, hi in self._organ_ranges}
        lane_steps = max(1, self._act_lane_steps)
        return {
            "artificial_neuron_count": self.n_extra,
            "artificial_edge_count": self.nnz_extra,
            "artificial_neuron_state_bytes": neuron_bytes,
            "artificial_edge_bytes": edge_bytes,
            "per_organ_buffer_bytes": organ_bytes,
            "genome_bytes": int(self.phenotype.get("genome_bytes") or 0),
            # canonical metrics for §8/§9. The compute cost counts edges
            # traversed in the *artificial* graph: base-graph traffic is
            # the shared substrate, and an organism is charged for the
            # circuit it added, not for the connectome it was born into.
            "marginal_memory_cost_bytes": neuron_bytes + edge_bytes,
            "marginal_compute_cost": round(
                self._act_edges_extra / lane_steps, 4),
            "active_propagated_edges": self._act_edges,
            "active_propagated_edges_artificial": self._act_edges_extra,
            "estimated_operations": self._act_edges * 2,
        }

    def get_population_activity(self, groups: list[str]) -> dict[str, list[float]]:
        rates = self._rates_hz()
        out = {}
        for gname in groups:
            if gname == "fba0":
                sl = slice(0, self.n_base)
            elif gname == "all":
                sl = slice(0, self.n)
            elif gname.startswith("organ:"):
                oid = gname.split(":", 1)[1]
                rng = next((r for r in self._organ_ranges if r[0] == oid), None)
                if rng is None:
                    out[gname] = [0.0] * self.batch_size
                    continue
                sl = slice(rng[1], rng[2])
            else:
                out[gname] = [0.0] * self.batch_size
                continue
            out[gname] = rates[:, sl].mean(1).tolist()
        return out

    # ------------------------------------------------------------ ckpt
    def checkpoint(self) -> bytes:
        state = {
            "t_ms": self.t_ms, "v": self.v.cpu(), "g": self.g.cpu(),
            "delay_buf": self.delay_buf.cpu(), "delay_ptr": self._delay_ptr,
            "refrac": self.refrac.cpu(),
            "spikes": self.spikes.cpu(),
            "spike_counts": self.spike_counts.cpu(),
            "gen_states": [g.get_state().cpu() for g in self._gens],
            "replicate_seeds": list(self.replicate_seeds),
            "build_gen_state": self._build_gen.get_state().cpu(),
            "force": self._force.cpu() if self._force is not None else None,
        }
        # NOTE: the global torch / CUDA RNG state is deliberately not
        # stored. Nothing in the evaluation path draws from it (every
        # draw goes through an explicit Generator), and restoring it
        # would be a cross-slot hazard once one process runs several
        # concurrent evaluations (M1 §2.4-6).
        return pickle.dumps(state)

    def restore(self, blob: bytes) -> None:
        s = pickle.loads(blob)
        self.t_ms = s["t_ms"]
        for k in ("v", "g", "delay_buf", "refrac", "spikes", "spike_counts"):
            setattr(self, k, s[k].to(self.device))
        self._delay_ptr = int(s.get("delay_ptr", 0))
        if len(s["gen_states"]) != len(self._gens):
            raise ValueError("checkpoint batch width differs from this backend")
        for g, st in zip(self._gens, s["gen_states"]):
            g.set_state(st)
        self.replicate_seeds = list(s["replicate_seeds"])
        self._build_gen.set_state(s["build_gen_state"])
        f = s.get("force")
        self._force = f.to(self.device) if f is not None else None

    def capabilities(self) -> dict:
        return {"supports_gpu": bool(torch.cuda.is_available()),
                "supports_batch": True, "is_reference": False,
                "max_batch_hint": 64}
