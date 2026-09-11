"""Worker startup benchmark: pick the GPU execution batch (lanes per
chunk). Purely operational — it changes wall time, never the replicate
set or fitness (see fba/replicates.py).

Each candidate is run on the *experiment's* execution profile (backend,
dataset/network size, duration, phenotype size — see
``bench_phenotype`` / ``profile_backend_kwargs``) and records allocated /
reserved / total VRAM, sim-s per wall-s and throughput
(= lanes x sim-s / wall-s). ``choose_batch`` keeps only rows that
succeeded and whose reserved VRAM stays under ``vram_headroom`` of the
device total, then takes the best throughput (smallest batch on ties).
"""
from __future__ import annotations

from dataclasses import dataclass, asdict

DEFAULT_CANDIDATES = (1, 2, 4, 8, 16, 32)
DEFAULT_VRAM_HEADROOM = 0.85
BENCH_ORGAN_ID = "bench-organ"


@dataclass
class BenchRow:
    batch: int
    sim_seconds_per_wall_second: float | None
    throughput: float | None            # lanes * sim-s / wall-s
    wall_s: float | None
    vram_allocated_bytes: int | None
    vram_reserved_bytes: int | None
    vram_total_bytes: int | None
    ok: bool
    error: str | None = None
    vram_bytes: int | None = None       # alias of allocated (legacy readers)

    @property
    def vram_fraction(self) -> float | None:
        if self.vram_reserved_bytes is None or not self.vram_total_bytes:
            return None
        return self.vram_reserved_bytes / self.vram_total_bytes

    def to_dict(self):
        d = asdict(self)
        d["vram_fraction"] = self.vram_fraction
        return d

    @classmethod
    def from_dict(cls, d: dict) -> "BenchRow":
        return cls(batch=int(d["batch"]),
                   sim_seconds_per_wall_second=d.get(
                       "sim_seconds_per_wall_second"),
                   throughput=d.get("throughput"),
                   wall_s=d.get("wall_s"),
                   vram_allocated_bytes=d.get("vram_allocated_bytes",
                                              d.get("vram_bytes")),
                   vram_reserved_bytes=d.get("vram_reserved_bytes"),
                   vram_total_bytes=d.get("vram_total_bytes"),
                   ok=bool(d.get("ok")), error=d.get("error"),
                   vram_bytes=d.get("vram_bytes"))


def profile_backend_kwargs(profile: dict, data_dir: str | None = None,
                           runs_dir: str | None = None) -> dict:
    """Backend constructor kwargs from a coordinator worker profile
    (GET /api/worker/profile): the same dataset/network the jobs use."""
    fba = dict(profile.get("fba") or {})
    kw = {
        "synthetic": bool(fba.get("synthetic", True)),
        "synthetic_neurons": int(fba.get("synthetic_neurons", 2000)),
        "connectivity": float(fba.get("connectivity", 0.01)),
        "n_neurons": int(fba.get("synthetic_neurons", 512)),
    }
    if fba.get("base_seed") is not None:
        kw["base_seed"] = int(fba["base_seed"])
    if fba.get("propagation_backend"):
        kw["propagation_backend"] = str(fba["propagation_backend"])
    if fba.get("dense_above") is not None:
        kw["dense_above"] = float(fba["dense_above"])
    if fba.get("synthetic_edges"):
        kw["synthetic_edges"] = int(fba["synthetic_edges"])
    if fba.get("region_mode"):
        kw["region_mode"] = fba["region_mode"]
    dd = data_dir or fba.get("data_dir")
    if dd:
        kw["data_dir"] = dd
    if runs_dir:
        kw["runs_dir"] = runs_dir
    return kw


def bench_phenotype(profile: dict) -> dict:
    """FBA0 phenotype plus the configured worst-case organ overhead
    (``worker.bench_organ_overhead_neurons``) so the chosen batch still
    fits once organisms grow."""
    from ..development.phenotype import develop
    from ..genome.schema import fba0_genome
    phen = develop(fba0_genome())
    extra = int((profile.get("worker") or {})
                .get("bench_organ_overhead_neurons", 0) or 0)
    if extra > 0:
        phen = dict(phen)
        phen["n_extra_neurons"] = int(phen.get("n_extra_neurons", 0)) + extra
        phen["artificial_organs"] = list(phen.get("artificial_organs", [])) + [
            {"organ_id": BENCH_ORGAN_ID, "size": extra, "kind": "bench"}]
    return phen


def _vram_of(backend) -> dict:
    summary = backend.get_state_summary()
    v = summary.get("vram") or {}
    alloc = v.get("allocated")
    if alloc is None:
        alloc = summary.get("vram_bytes")
    return {"allocated": alloc, "reserved": v.get("reserved"),
            "total": v.get("total")}


def startup_benchmark(backend, phenotype, device,
                      candidates=DEFAULT_CANDIDATES,
                      duration_ms: float = 200, drive: dict | None = None
                      ) -> list[BenchRow]:
    """Run each candidate batch once; stop increasing after a failure."""
    rows: list[BenchRow] = []
    for batch in candidates:
        try:
            backend.initialize(phenotype, batch_size=batch, seed=0,
                               device=device)
            if drive:
                backend.set_inputs(drive)
            stats = backend.run(duration_ms)
            sim_s = stats["simulated_ms"] / 1000.0
            wall = float(stats["wall_s"])
            per_wall = sim_s / wall if wall > 0 else None
            vram = _vram_of(backend)
            rows.append(BenchRow(
                batch=int(batch), sim_seconds_per_wall_second=per_wall,
                throughput=(per_wall * batch) if per_wall else None,
                wall_s=wall,
                vram_allocated_bytes=vram["allocated"],
                vram_reserved_bytes=vram["reserved"],
                vram_total_bytes=vram["total"], ok=True,
                vram_bytes=vram["allocated"]))
        except Exception as exc:  # OOM or other failure -> stop increasing
            rows.append(BenchRow(
                batch=int(batch), sim_seconds_per_wall_second=None,
                throughput=None, wall_s=None, vram_allocated_bytes=None,
                vram_reserved_bytes=None, vram_total_bytes=None, ok=False,
                error=f"{type(exc).__name__}: {exc}"))
            break
    return rows


def choose_batch(rows: list[BenchRow],
                 vram_headroom: float = DEFAULT_VRAM_HEADROOM,
                 max_batch: int | None = None) -> int:
    """VRAM-aware selection.

    Eligible = ok rows with ``reserved / total <= vram_headroom`` (rows
    without VRAM data — CPU — are eligible) and ``batch <= max_batch``.
    Pick the eligible row with the highest throughput; ties go to the
    smaller batch (more VRAM slack for the same speed). Falls back to 1.
    """
    eligible = []
    for r in rows:
        if not r.ok:
            continue
        if max_batch is not None and r.batch > max_batch:
            continue
        frac = r.vram_fraction
        if frac is not None and frac > vram_headroom:
            continue
        eligible.append(r)
    if not eligible:
        return 1
    return min(eligible,
               key=lambda r: (-(r.throughput or 0.0), r.batch)).batch
