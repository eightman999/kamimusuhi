"""Torch FBA backend — our own implementation of the Shiu et al. (2024)
Drosophila LIF model (read equations from the GPL fly-brain repo only;
no code copied).

Model: Poisson drive (scalePoisson=250, wScale=0.275) -> recurrent
sparse weights -> alpha synapse (tauSyn=5 ms) with a 1.8 ms delay ring
buffer -> LIF (tauMem=20 ms, vRest=-52 mV, vThr=-45 mV, refractory
2.2 ms) at dt=0.1 ms.

Connectivity convention
-----------------------
``self.W`` is an ``N x N`` sparse CSR tensor indexed ``W[post, pre]``
(row = postsynaptic, column = presynaptic). Propagation is

    I_post = W @ spikes_pre        i.e.  torch.sparse.mm(W, spikes.T).T

so a spike of neuron ``pre`` delivers ``W[post, pre]`` to ``post``.
(``spikes @ W`` would propagate post -> pre and, for CSR, torch returns
zeros silently — never use it.)

The matrix is built sparse-only: FBA0 edges (COO from the parquet, or a
sampled synthetic edge list), artificial-organ internal edges, and
attachment edges are concatenated as index/value lists and coalesced
into one CSR. No ``N x N`` dense tensor is ever allocated, so memory is
``O(E_fba0 + E_organs + E_attachments)``.

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
(CPU, seeded from ``seed``) for network construction and ``self._gen``
(on ``device``) for the per-step Poisson drive. ``checkpoint()`` stores
both generator states plus the simulation state, so replaying the same
steps after ``restore()`` is bit-identical on CPU. On CUDA, sparse matmul
may use non-deterministic atomics; see README "Determinism".
"""
from __future__ import annotations

import hashlib
import os
import pickle
import time
from pathlib import Path

import numpy as np

from .backend import BackendUnavailable, FbaBackend
from .fba0 import DATA_FILES
from .params import DEFAULT_PARAMS, UnsupportedAttachmentRegion

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
                  no_self=True):
    """Sample ~p*|post|*|pre| directed edges pre->post uniformly (with
    replacement; duplicates are summed by coalesce) as (post, pre)
    int64 tensors. O(edges) memory."""
    n_post = n_post_hi - n_post_lo
    n_pre = n_pre_hi - n_pre_lo
    m = int(round(p * n_post * n_pre))
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
                 runs_dir: str | None = None, region_mode: str | None = None):
        if torch is None:
            raise BackendUnavailable("torch not installed")
        self.data_dir = data_dir or os.environ.get("MIOBA_FLY_BRAIN_DATA")
        self.synthetic = synthetic
        self.synthetic_neurons = int(synthetic_neurons)
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
        return {"dataset_id": "synthetic-fba",
                "version": f"v0-n{self.synthetic_neurons}-p{self.connectivity}",
                "manifest_hash": None,
                "region_mode": self.region_mode}

    # ------------------------------------------------------------ init
    def initialize(self, phenotype: dict, batch_size: int, seed: int,
                   device: str) -> None:
        self.batch_size = int(batch_size)
        self.seed = int(seed)
        self.device = torch.device(device if device else "cpu")
        self.phenotype = phenotype or {}
        self._build_gen = torch.Generator(device="cpu").manual_seed(self.seed)
        self._gen = torch.Generator(device=self.device.type).manual_seed(
            self.seed)

        self.params = dict(PARAMS)
        self.params.update(self.phenotype.get("params") or {})

        cache = (self.runs_dir / "cache") if self.runs_dir else None
        if self.data_dir:
            n_base, post, pre, w, manifest = _load_connectome(
                Path(self.data_dir), cache)
            self._manifest_hash = manifest
            self.region_mode = self._region_mode_arg
        elif self.synthetic:
            n_base = self.synthetic_neurons
            post, pre = _sample_edges(0, n_base, 0, n_base, self.connectivity,
                                      self._build_gen)
            w = torch.rand(post.numel(), generator=self._build_gen) \
                * self.params["wScale"]
            self.region_mode = self._region_mode_arg or SYNTHETIC_REGION_MODE
        else:
            raise BackendUnavailable(
                "no fba.data_dir configured and synthetic=False")

        n_extra = int(self.phenotype.get("n_extra_neurons", 0) or 0)
        self.n_base = n_base
        self.n = n_base + n_extra
        self._organ_ranges: list[tuple[str, int, int]] = []
        off = n_base
        for organ in self.phenotype.get("artificial_organs", []):
            self._organ_ranges.append((organ["organ_id"], off,
                                       off + int(organ["size"])))
            off += int(organ["size"])
        if off != self.n:
            raise ValueError("n_extra_neurons != sum(organ sizes)")

        self.n_edges_base = int(post.numel())
        post, pre, w = self._with_organs(post, pre, w, self._build_gen)
        self.n_edges_total = int(post.numel())
        idx = torch.stack([post, pre])
        W = torch.sparse_coo_tensor(idx, w, (self.n, self.n)).coalesce()
        self.W = W.to_sparse_csr().to(self.device)
        self.nnz = int(W._nnz())

        self._input_rates = torch.zeros(self.n, device=self.device)
        self._silence = torch.zeros(self.n, dtype=torch.bool, device=self.device)
        self.steps_delay = max(1, int(round(self.params["tDelay"] /
                                            self.params["dt"])))
        self.reset()

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

    def _with_organs(self, post, pre, w, gen):
        """Append organ-internal and attachment edges (sparse index lists).

        Organ internal: p=ORGAN_INTERNAL_P within each organ block.
        Attachment: source (pre) -> target (post) pairs at p=ATTACHMENT_P
        with weight wScale*weight_scale; 'bidirectional' also adds
        target -> source. Endpoints are resolved by name; nothing is
        wired at random across the whole network.
        """
        if not self._organ_ranges:
            return post, pre, w
        posts, pres, ws = [post], [pre], [w]
        wscale = float(self.params["wScale"])
        for _oid, lo, hi in self._organ_ranges:
            p_, r_ = _sample_edges(lo, hi, lo, hi, ORGAN_INTERNAL_P, gen)
            posts.append(p_)
            pres.append(r_)
            ws.append(torch.rand(p_.numel(), generator=gen) * wscale)
        for att in self.phenotype.get("attachments", []):
            s_lo, s_hi = self._endpoint_range(att["source"])
            t_lo, t_hi = self._endpoint_range(att["target"])
            scale = wscale * float(att.get("weight_scale", 1.0))
            legs = [((t_lo, t_hi), (s_lo, s_hi))]
            if att.get("direction", "forward") == "bidirectional":
                legs.append(((s_lo, s_hi), (t_lo, t_hi)))
            for (plo, phi), (rlo, rhi) in legs:
                p_, r_ = _sample_edges(plo, phi, rlo, rhi, ATTACHMENT_P, gen)
                posts.append(p_)
                pres.append(r_)
                ws.append(torch.rand(p_.numel(), generator=gen) * scale)
        return torch.cat(posts), torch.cat(pres), torch.cat(ws)

    # ------------------------------------------------------------ state
    def reset(self) -> None:
        p = self.params
        d, B, n = self.device, self.batch_size, self.n
        self.t_ms = 0.0
        self.v = torch.full((B, n), p["v0"], device=d)
        self.g = torch.zeros((B, n), device=d)
        self.delay_buf = torch.zeros((B, self.steps_delay + 1, n), device=d)
        self.refrac = torch.full((B, n), p["tRefrac"], device=d)
        self.spikes = torch.zeros((B, n), device=d)
        self.spike_counts = torch.zeros((B, n), dtype=torch.long, device=d)
        self._gen.manual_seed(self.seed)

    def set_inputs(self, drive: dict) -> None:
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
        self._silence = torch.zeros(self.n, dtype=torch.bool, device=self.device)
        for i in drive.get("silence") or []:
            self._silence[int(i)] = True

    def force_spikes(self, neuron_ids: list[int] | None) -> None:
        """Debug/test hook: force these neurons to spike every step."""
        if not neuron_ids:
            self._force = None
            return
        f = torch.zeros(self.n, dtype=torch.bool, device=self.device)
        for i in neuron_ids:
            f[int(i)] = True
        self._force = f

    def propagate(self, spikes: torch.Tensor) -> torch.Tensor:
        """Synaptic input to every post neuron given a (B, N) spike
        matrix: I[b, post] = sum_pre W[post, pre] * spikes[b, pre]."""
        return torch.sparse.mm(self.W, spikes.T).T

    # ------------------------------------------------------------ step
    def _one_step(self) -> None:
        p, d = self.params, self.device
        dt = p["dt"]
        u = torch.rand((self.batch_size, self.n), device=d, generator=self._gen)
        stim = (u < self._input_rates[None, :] * dt / 1000.0).float() \
            * p["scalePoisson"]
        delayed = self.delay_buf[:, 0, :]
        self.delay_buf = torch.roll(self.delay_buf, -1, dims=1)
        self.delay_buf[:, -1, :] = self.propagate(self.spikes)
        active = (self.refrac >= p["tRefrac"]).float()
        self.g = self.g * (1 - dt / p["tauSyn"]) + delayed * active
        self.v = self.v + stim + (dt / p["tauMem"]) * (self.g - (self.v - p["vRest"]))
        spikes = ((self.v >= p["vThr"]) & (self.refrac >= p["tRefrac"])).float()
        if self._force is not None:
            spikes[:, self._force] = 1.0
        spikes[:, self._silence] = 0.0
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
            "n_neurons": self.n,
            "n_base": self.n_base,
            "nnz": self.nnz,
            "region_mode": self.region_mode,
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
            "delay_buf": self.delay_buf.cpu(), "refrac": self.refrac.cpu(),
            "spikes": self.spikes.cpu(),
            "spike_counts": self.spike_counts.cpu(),
            "gen_state": self._gen.get_state().cpu(),
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
        self._gen.set_state(s["gen_state"])
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
