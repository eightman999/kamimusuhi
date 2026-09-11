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
The ``tDelay`` ring buffer is indexed by ``self._delay_ptr`` rather than
rolled. M0 called ``torch.roll`` on a ``(B, steps_delay+1, N)`` tensor
every step, i.e. it copied ~10 MB per lane per step (≈50 GB of memory
traffic over one 500 ms evaluation at FlyWire scale) to achieve what a
pointer increment does. The read/write order is identical.

Poisson drive
-------------
``set_inputs`` keeps only the neurons whose rate is non-zero
(``self._drive_idx``). M0 drew ``N`` uniforms per lane per step and
compared them against a vector that was zero for 99% of entries; M1 draws
one uniform per *driven* neuron. The stimulus is applied with the same
``scalePoisson`` to the same neurons, but the random stream is consumed
differently, so M1 trajectories are not comparable to M0 ones — as with
the topology change, this is a new ``scientific_config_hash``.

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
import os
import pickle
import time
from pathlib import Path

import numpy as np

from ..perf import PhaseTimer
from .backend import BackendUnavailable, FbaBackend
from .eventgraph import EventGraph
from .fba0 import DATA_FILES
from .params import DEFAULT_PARAMS, UnsupportedAttachmentRegion
from .replicates import replicate_seeds as _default_replicate_seeds
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


class TorchBackend(FbaBackend):
    name = "torch"

    def __init__(self, data_dir: str | None = None, synthetic: bool = True,
                 synthetic_neurons: int = 2000, connectivity: float = 0.01,
                 runs_dir: str | None = None, region_mode: str | None = None,
                 synthetic_edges: int | None = None, base_seed: int = 0,
                 topology_cache: bool = True):
        if torch is None:
            raise BackendUnavailable("torch not installed")
        self.data_dir = data_dir or os.environ.get("MIOBA_FLY_BRAIN_DATA")
        self.synthetic = synthetic
        self.synthetic_neurons = int(synthetic_neurons)
        # seed of the *shared* FBA0 reference graph (M1): one base network
        # for the whole run, independent of any genome's random_seed
        self.base_seed = int(base_seed)
        self.use_topology_cache = bool(topology_cache)
        # explicit edge count (FlyWire-scale smoke: 139k neurons / ~14M
        # edges) wins over the pair probability
        self.synthetic_edges = int(synthetic_edges) if synthetic_edges else None
        self.connectivity = float(connectivity)
        self.runs_dir = Path(runs_dir) if runs_dir else None
        # explicit region_mode wins; synthetic defaults to the pseudo-region
        # partition, real data to None (=> no fba0:<region> attachments)
        self._region_mode_arg = region_mode
        self.region_mode = region_mode or (SYNTHETIC_REGION_MODE
                                           if not self.data_dir else None)
        self._manifest_hash: str | None = None
        self._force: torch.Tensor | None = None

    # ------------------------------------------------------------ identity
    def dataset_identity(self) -> dict:
        """Logical dataset identity for the research record (never a raw
        path): id, version, manifest hash, region mode."""
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

        self._load_base(timer)

        n_extra = int(self.phenotype.get("n_extra_neurons", 0) or 0)
        self.n = self.n_base + n_extra
        self.n_extra = n_extra
        self._organ_ranges: list[tuple[str, int, int]] = []
        off = self.n_base
        for organ in self.phenotype.get("artificial_organs", []):
            self._organ_ranges.append((organ["organ_id"], off,
                                       off + int(organ["size"])))
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
            self.steps_delay = max(1, int(round(self.params["tDelay"] /
                                                self.params["dt"])))
            self.reset()

    # ------------------------------------------------------------ base graph
    def _load_base(self, timer: PhaseTimer) -> None:
        """Build (or reuse) the resident FBA0 reference graph.

        Values are kept raw; ``self._base_scale`` is applied at
        propagation time so that a ``wScale`` mutation is a scalar
        multiply, not a 14M-edge rebuild.
        """
        cache_dir = (self.runs_dir / "cache") if self.runs_dir else None
        if self.data_dir:
            self.region_mode = self._region_mode_arg
        elif self.synthetic:
            self.region_mode = self._region_mode_arg or SYNTHETIC_REGION_MODE
        else:
            raise BackendUnavailable(
                "no fba.data_dir configured and synthetic=False")

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
            if self.data_dir:
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
            add(*_sample_edges(lo, hi, lo, hi, ORGAN_INTERNAL_P, gen), 0)
        for i, att in enumerate(self.phenotype.get("attachments", [])):
            s_lo, s_hi = self._endpoint_range(att["source"])
            t_lo, t_hi = self._endpoint_range(att["target"])
            legs = [((t_lo, t_hi), (s_lo, s_hi))]
            if att.get("direction", "forward") == "bidirectional":
                legs.append(((s_lo, s_hi), (t_lo, t_hi)))
            for (plo, phi), (rlo, rhi) in legs:
                add(*_sample_edges(plo, phi, rlo, rhi, ATTACHMENT_P, gen),
                    i + 1)
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
        self.delay_buf = torch.zeros((B, self.steps_delay + 1, n), device=d)
        # ring-buffer read/write slot (M0 rolled the whole buffer instead)
        self._delay_ptr = 0
        self.refrac = torch.full((B, n), p["tRefrac"], device=d)
        self.spikes = torch.zeros((B, n), device=d)
        self.spike_counts = torch.zeros((B, n), dtype=torch.long, device=d)
        for g, s in zip(self._gens, self.replicate_seeds):
            g.manual_seed(s)

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
        out = torch.zeros((spikes.shape[0], self.n), device=spikes.device,
                          dtype=spikes.dtype)
        self.W_base.propagate(spikes, out, scale=self._base_scale,
                              col_limit=self.n_base)
        if self.W_extra is not None:
            self.W_extra.propagate(spikes, out)
        return out

    # ------------------------------------------------------------ step
    def _one_step(self) -> None:
        p, d = self.params, self.device
        dt = p["dt"]
        L = self.delay_buf.shape[1]
        ptr = self._delay_ptr
        delayed = self.delay_buf[:, ptr, :]
        active = (self.refrac >= p["tRefrac"]).float()
        self.g = self.g * (1 - dt / p["tauSyn"]) + delayed * active
        self.delay_buf[:, ptr, :] = self.propagate(self.spikes)
        self._delay_ptr = (ptr + 1) % L
        self.v = self.v + (dt / p["tauMem"]) * (self.g - (self.v - p["vRest"]))
        if self._drive_idx is not None:
            u = torch.stack([torch.rand((self._drive_idx.numel(),), device=d,
                                        generator=g) for g in self._gens])
            self.v[:, self._drive_idx] += \
                (u < self._drive_p[None, :]).to(self.v.dtype) * p["scalePoisson"]
        spikes = ((self.v >= p["vThr"]) & (self.refrac >= p["tRefrac"])).float()
        if self._force is not None:
            spikes[:, self._force] = 1.0
        if self._silence_idx is not None:
            spikes[:, self._silence_idx] = 0.0
        self.v = torch.where(spikes > 0, torch.full_like(self.v, p["vReset"]),
                             self.v)
        self.g = self.g * (1 - spikes)  # reset conductance of spiking cells
        self.refrac = torch.where(spikes > 0, torch.zeros_like(self.refrac),
                                  self.refrac + dt)
        self.spikes = spikes
        self.spike_counts += spikes.long()
        self.t_ms += dt

    def step(self, n_steps: int = 1) -> None:
        with torch.no_grad():
            for _ in range(int(n_steps)):
                self._one_step()

    def run(self, duration_ms: float) -> dict:
        n = max(1, int(round(duration_ms / self.params["dt"])))
        t0 = time.perf_counter()
        self.step(n)
        wall = time.perf_counter() - t0
        return {"simulated_ms": float(n * self.params["dt"]),
                "wall_s": wall,
                "spikes_total": int(self.spike_counts.sum().item())}

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
            "torch_rng_state": torch.get_rng_state(),
            "cuda_rng_state": (torch.cuda.get_rng_state(self.device)
                               if self.device.type == "cuda" else None),
            "force": self._force.cpu() if self._force is not None else None,
        }
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
        torch.set_rng_state(s["torch_rng_state"])
        if s.get("cuda_rng_state") is not None and self.device.type == "cuda":
            torch.cuda.set_rng_state(s["cuda_rng_state"], self.device)
        f = s.get("force")
        self._force = f.to(self.device) if f is not None else None

    def capabilities(self) -> dict:
        return {"supports_gpu": bool(torch.cuda.is_available()),
                "supports_batch": True, "is_reference": False,
                "max_batch_hint": 64}
