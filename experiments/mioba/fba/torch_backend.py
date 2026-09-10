"""Torch FBA backend — our own implementation of the Shiu et al. (2024)
Drosophila LIF model (read equations from the GPL fly-brain repo only;
no code copied).

Model: Poisson drive (scalePoisson=250, wScale=0.275) -> recurrent
sparse weights -> alpha synapse (tauSyn=5 ms) with a 1.8 ms delay ring
buffer -> LIF (tauMem=20 ms, vRest=-52 mV, vThr=-45 mV, refractory
2.2 ms) at dt=0.1 ms.

Connectivity is loaded from a fly-brain ``data/`` directory pointed to
by the ``MIOBA_FLY_BRAIN_DATA`` env var or config ``fba.data_dir``:
``2025_Completeness_783.csv`` (FlyWire root ids) and
``2025_Connectivity_783.parquet`` (Presynaptic_Index,
Postsynaptic_Index, "Excitatory x Connectivity" columns). pandas+pyarrow
are optional imports — without them loading raises BackendUnavailable.
Weights are cached as a torch CSR tensor under
``<runs>/cache/weights_<sha256(size,mtime)>.pt``.

With no data dir, ``synthetic=True`` gives a random sparse N=2000
network so the backend can be exercised on CPU.
"""
from __future__ import annotations

import hashlib
import os
import pickle
import time
from pathlib import Path

import numpy as np

from .backend import BackendUnavailable, FbaBackend

try:
    import torch
except ImportError:  # pragma: no cover
    torch = None

PARAMS = dict(tauMem=20.0, tauSyn=5.0, tDelay=1.8, v0=-52.0, vReset=-52.0,
              vRest=-52.0, vThr=-45.0, tRefrac=2.2, dt=0.1,
              scalePoisson=250.0, wScale=0.275)


def _load_connectome(data_dir: Path, cache_dir: Path | None):
    """Return (n_neurons, scipy-style coo -> torch sparse csr tensor)."""
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
        return blob["n"], blob["W"]

    df = pd.read_parquet(con_path)
    col_pre = "Presynaptic_Index" if "Presynaptic_Index" in df.columns else df.columns[0]
    col_post = "Postsynaptic_Index" if "Postsynaptic_Index" in df.columns else df.columns[1]
    col_w = ("Excitatory x Connectivity"
             if "Excitatory x Connectivity" in df.columns else df.columns[-1])
    pre = df[col_pre].to_numpy()
    post = df[col_post].to_numpy()
    w = df[col_w].to_numpy().astype(np.float32)
    n = int(max(pre.max(initial=0), post.max(initial=0)) + 1)
    if comp_path.is_file():
        n = max(n, sum(1 for _ in comp_path.open("rb")) - 1)
    idx = torch.from_numpy(np.stack([post, pre]).astype(np.int64))
    W = torch.sparse_coo_tensor(idx, torch.from_numpy(w), (n, n))
    W = W.to_sparse_csr()
    if cache_file:
        cache_file.parent.mkdir(parents=True, exist_ok=True)
        torch.save({"n": n, "W": W}, cache_file)
    return n, W


class TorchBackend(FbaBackend):
    name = "torch"

    def __init__(self, data_dir: str | None = None, synthetic: bool = True,
                 synthetic_neurons: int = 2000, connectivity: float = 0.01,
                 runs_dir: str | None = None):
        if torch is None:
            raise BackendUnavailable("torch not installed")
        self.data_dir = data_dir or os.environ.get("MIOBA_FLY_BRAIN_DATA")
        self.synthetic = synthetic
        self.synthetic_neurons = int(synthetic_neurons)
        self.connectivity = connectivity
        self.runs_dir = Path(runs_dir) if runs_dir else None

    # ------------------------------------------------------------ init
    def initialize(self, phenotype: dict, batch_size: int, seed: int,
                   device: str) -> None:
        self.batch_size = int(batch_size)
        self.seed = int(seed)
        self.device = torch.device(device if device else "cpu")
        self.phenotype = phenotype or {}
        gen = torch.Generator(device="cpu").manual_seed(self.seed)

        cache = (self.runs_dir / "cache") if self.runs_dir else None
        if self.data_dir:
            n_base, W_base = _load_connectome(Path(self.data_dir), cache)
        elif self.synthetic:
            n_base = self.synthetic_neurons
            W_base = self._random_weights(n_base, self.connectivity, gen)
        else:
            raise BackendUnavailable(
                "no fba.data_dir configured and synthetic=False")

        self.params = dict(PARAMS)
        for k, scale in (self.phenotype.get("param_overrides") or {}).items():
            if k in self.params:
                self.params[k] *= scale

        # append artificial organs
        n_extra = int(self.phenotype.get("n_extra_neurons", 0) or 0)
        self.n_base = n_base
        self.n = n_base + n_extra
        self._organ_ranges = []
        off = n_base
        for organ in self.phenotype.get("artificial_organs", []):
            self._organ_ranges.append((organ["organ_id"], off,
                                       off + int(organ["size"])))
            off += int(organ["size"])
        W = self._with_organs(W_base, gen)
        self.W = W.to(self.device)
        self._gen = gen
        self._input_rates = torch.zeros(self.n, device=self.device)
        self._silence = torch.zeros(self.n, dtype=torch.bool, device=self.device)
        self.steps_delay = max(1, int(round(self.params["tDelay"] /
                                            self.params["dt"])))
        self.reset()

    def _random_weights(self, n, p, gen):
        mask = torch.rand((n, n), generator=gen) < p
        mask.fill_diagonal_(False)
        w = mask * (torch.rand((n, n), generator=gen) * self.params_wscale())
        return w.to_sparse_csr()

    def params_wscale(self):
        return PARAMS["wScale"]

    def _with_organs(self, W_base, gen):
        """Append random sparse organ blocks scaled by attachment weight_scale."""
        n_extra = self.n - self.n_base
        if n_extra <= 0:
            return W_base
        W = torch.zeros((self.n, self.n))
        W[: self.n_base, : self.n_base] = W_base.to_dense()
        scales = [1.0]
        for att in self.phenotype.get("attachments", []):
            try:
                scales.append(float(att.get("weight_scale", 1.0)))
            except (TypeError, ValueError):
                pass
        s = float(np.mean(scales))
        extra = torch.rand((self.n, self.n), generator=gen) < 0.01
        block = extra * torch.rand((self.n, self.n), generator=gen) * \
            self.params_wscale() * s
        # organs connect sparsely both ways, not within fba0 block
        block[: self.n_base, : self.n_base] = 0
        W += block
        return W.to_sparse_csr()

    # ------------------------------------------------------------ state
    def reset(self) -> None:
        p = self.params
        d, B, n = self.device, self.batch_size, self.n
        self.t_ms = 0.0
        self.v = torch.full((B, n), p["v0"], device=d)
        self.g = torch.zeros((B, n), device=d)
        self.delay_buf = torch.zeros((B, self.steps_delay + 1, n), device=d)
        self.refrac = torch.zeros((B, n), device=d)
        self.spikes = torch.zeros((B, n), device=d)
        self.spike_counts = torch.zeros((B, n), dtype=torch.long, device=d)
        self._gen_state = self._gen.get_state()

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

    # ------------------------------------------------------------ step
    def _one_step(self) -> None:
        p, d = self.params, self.device
        dt = p["dt"]
        stim = (torch.rand((self.batch_size, self.n), device=d)
                < self._input_rates[None, :] * dt / 1000.0).float() \
            * p["scalePoisson"]
        delayed = self.delay_buf[:, 0, :]
        self.delay_buf = torch.roll(self.delay_buf, -1, dims=1)
        self.delay_buf[:, -1, :] = self.spikes @ self.W
        active = (self.refrac >= p["tRefrac"]).float()
        self.g = self.g * (1 - dt / p["tauSyn"]) + delayed * active
        self.v = self.v + stim + (dt / p["tauMem"]) * (self.g - (self.v - p["vRest"]))
        spikes = ((self.v >= p["vThr"]) & (self.refrac >= p["tRefrac"])).float()
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
        return pickle.dumps({
            "t_ms": self.t_ms, "v": self.v.cpu(), "g": self.g.cpu(),
            "delay_buf": self.delay_buf.cpu(), "refrac": self.refrac.cpu(),
            "spikes": self.spikes.cpu(),
            "spike_counts": self.spike_counts.cpu(),
        })

    def restore(self, blob: bytes) -> None:
        s = pickle.loads(blob)
        self.t_ms = s["t_ms"]
        for k in ("v", "g", "delay_buf", "refrac", "spikes", "spike_counts"):
            setattr(self, k, s[k].to(self.device))

    def capabilities(self) -> dict:
        return {"supports_gpu": bool(torch.cuda.is_available()),
                "supports_batch": True, "is_reference": False,
                "max_batch_hint": 64}
