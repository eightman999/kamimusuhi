"""Mock FBA backend: a tiny numpy sparse LIF network.

Same parameter names as Shiu et al. (2024) (tauMem=20 ms, tauSyn=5 ms,
vRest=-52 mV, vThr=-45 mV, tRefrac=2.2 ms, dt=0.1 ms). Deterministic
under a given seed. Used by all tests and the smoke harness.
"""
from __future__ import annotations

import pickle
import time

import numpy as np

from .backend import BackendUnavailable, FbaBackend
from .params import DEFAULT_PARAMS
from .replicates import replicate_seeds as _default_replicate_seeds

PARAMS = dict(DEFAULT_PARAMS)


class MockBackend(FbaBackend):
    name = "mock"

    def __init__(self, n_neurons: int = 512, connectivity: float = 0.02):
        self.n_neurons = int(n_neurons)
        self.connectivity = float(connectivity)
        self._rng_state = None

    # ------------------------------------------------------------- init
    def initialize(self, phenotype: dict, batch_size: int, seed: int,
                   device: str, replicate_seeds: list[int] | None = None
                   ) -> None:
        del device  # cpu-only
        self.batch_size = int(batch_size)
        self.seed = int(seed)
        if replicate_seeds is None:
            replicate_seeds = _default_replicate_seeds(self.seed, self.batch_size)
        if len(replicate_seeds) != self.batch_size:
            raise ValueError("len(replicate_seeds) must equal batch_size")
        self.replicate_seeds = [int(s) for s in replicate_seeds]
        self.phenotype = phenotype or {}
        n_extra = int(self.phenotype.get("n_extra_neurons", 0) or 0)
        self.n = self.n_neurons + n_extra
        self._organ_ranges = []
        off = self.n_neurons
        for organ in self.phenotype.get("artificial_organs", []):
            self._organ_ranges.append((organ["organ_id"], off, off + organ["size"]))
            off += organ["size"]

        # phenotype-resolved params (global scope; see fba/params.py)
        self.params = dict(PARAMS)
        self.params.update(self.phenotype.get("params") or {})
        rng = np.random.default_rng(self.seed)
        # W[pre, post]; propagation is spikes @ W (dense, small N only)
        mask = rng.random((self.n, self.n)) < self.connectivity
        np.fill_diagonal(mask, False)
        self.W = (mask * rng.uniform(0.1, 1.0, (self.n, self.n))
                  * self.params["wScale"]).astype(np.float32)
        self._input_rates = np.zeros(self.n, dtype=np.float32)
        self._silence = np.zeros(self.n, dtype=bool)
        self.reset()

    def dataset_identity(self) -> dict:
        return {"dataset_id": "mock-fba",
                "version": f"v0-n{self.n_neurons}-p{self.connectivity}",
                "manifest_hash": None, "region_mode": None}

    # ------------------------------------------------------------- state
    def reset(self) -> None:
        p = getattr(self, "params", PARAMS)
        self.t_ms = 0.0
        self.v = np.full((self.batch_size, self.n), p["vRest"], np.float32)
        self.g = np.zeros((self.batch_size, self.n), np.float32)
        self.refrac = np.zeros((self.batch_size, self.n), np.float32)
        self.spikes = np.zeros((self.batch_size, self.n), np.float32)
        self.spike_counts = np.zeros((self.batch_size, self.n), np.int64)
        # one Poisson-drive stream per lane, seeded by its replicate seed
        self._rngs = [np.random.default_rng(s) for s in self.replicate_seeds]

    def set_inputs(self, drive: dict) -> None:
        rates = np.zeros(self.n, np.float32)
        for key, hz in (drive.get("rates_hz") or {}).items():
            if isinstance(key, str) and key.startswith("slice:"):
                lo, hi = key.split(":")[1].split("-")
                rates[int(lo):int(hi)] = hz
            elif isinstance(key, str) and key == "all":
                rates[:] = hz
            else:
                rates[int(key)] = hz
        self._input_rates = rates
        self._silence = np.zeros(self.n, bool)
        for i in drive.get("silence") or []:
            self._silence[int(i)] = True

    # ------------------------------------------------------------- step
    def _one_step(self) -> None:
        p = self.params
        dt = p["dt"]
        # Poisson drive
        u = np.stack([r.random(self.n) for r in self._rngs])
        stim = (u < self._input_rates[None, :] * dt / 1000.0
                ).astype(np.float32) * 25.0
        # recurrent input through alpha-ish conductance decay
        rec = self.spikes @ self.W
        self.g = self.g * (1 - dt / p["tauSyn"]) + rec * 10.0
        self.v += stim + (dt / p["tauMem"]) * (self.g - (self.v - p["vRest"]))
        refractory = self.refrac > 0
        self.v[refractory] = p["vRest"]
        spikes = ((self.v >= p["vThr"]) & ~refractory).astype(np.float32)
        spikes[:, self._silence] = 0.0
        self.v = np.where(spikes > 0, p["vRest"], self.v)
        self.refrac = np.where(spikes > 0, p["tRefrac"],
                               np.maximum(self.refrac - dt, 0))
        self.spikes = spikes
        self.spike_counts += spikes.astype(np.int64)
        self.t_ms += dt

    def step(self, n_steps: int = 1) -> None:
        for _ in range(int(n_steps)):
            self._one_step()

    def run(self, duration_ms: float) -> dict:
        n = max(1, int(round(duration_ms / self.params["dt"])))
        t0 = time.perf_counter()
        self.step(n)
        wall = time.perf_counter() - t0
        return {"simulated_ms": float(n * self.params["dt"]),
                "wall_s": wall,
                "spikes_total": int(self.spike_counts.sum())}

    # ------------------------------------------------------------- report
    def _rates_hz(self) -> np.ndarray:
        t_s = max(self.t_ms, 1e-9) / 1000.0
        return self.spike_counts / t_s

    def get_state_summary(self) -> dict:
        rates = self._rates_hz()
        per_batch = rates.mean(axis=1)
        return {
            "t_ms": self.t_ms,
            "mean_rate_hz": float(rates.mean()),
            "active_fraction": float((self.spike_counts.sum(axis=1) > 0).mean()),
            "per_batch_spike_counts": self.spike_counts.sum(axis=1).tolist(),
            "per_batch_mean_rate_hz": per_batch.tolist(),
            "vram_bytes": None,
            "vram": {"allocated": None, "reserved": None, "total": None},
            "replicate_seeds": list(self.replicate_seeds),
        }

    def get_population_activity(self, groups: list[str]) -> dict[str, list[float]]:
        rates = self._rates_hz()
        out = {}
        for gname in groups:
            if gname == "fba0":
                sl = slice(0, self.n_neurons)
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
            out[gname] = rates[:, sl].mean(axis=1).tolist()
        return out

    # ------------------------------------------------------------- ckpt
    def checkpoint(self) -> bytes:
        return pickle.dumps({
            "t_ms": self.t_ms, "v": self.v, "g": self.g,
            "refrac": self.refrac, "spikes": self.spikes,
            "spike_counts": self.spike_counts,
            "rng": [r.bit_generator.state for r in self._rngs],
            "replicate_seeds": list(self.replicate_seeds),
        })

    def restore(self, blob: bytes) -> None:
        s = pickle.loads(blob)
        self.t_ms = s["t_ms"]
        self.v = s["v"]; self.g = s["g"]; self.refrac = s["refrac"]
        self.spikes = s["spikes"]; self.spike_counts = s["spike_counts"]
        if len(s["rng"]) != len(self._rngs):
            raise ValueError("checkpoint batch width differs from this backend")
        for r, st in zip(self._rngs, s["rng"]):
            r.bit_generator.state = st
        self.replicate_seeds = list(s["replicate_seeds"])

    def capabilities(self) -> dict:
        return {"supports_gpu": False, "supports_batch": True,
                "is_reference": False, "max_batch_hint": 8}
