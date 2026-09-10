"""Worker startup benchmark: pick a viable population batch size."""
from __future__ import annotations

from dataclasses import dataclass, asdict


@dataclass
class BenchRow:
    batch: int
    sim_seconds_per_wall_second: float | None
    vram_bytes: int | None
    ok: bool
    error: str | None = None

    def to_dict(self):
        return asdict(self)


def startup_benchmark(backend, phenotype, device, candidates=(1, 2, 4, 8, 16, 32),
                      duration_ms: float = 200) -> list[BenchRow]:
    """Run each candidate batch size once; stop increasing after a failure."""
    rows: list[BenchRow] = []
    for batch in candidates:
        try:
            backend.initialize(phenotype, batch_size=batch, seed=0,
                               device=device)
            stats = backend.run(duration_ms)
            sim_s = stats["simulated_ms"] / 1000.0
            per_wall = sim_s / stats["wall_s"] if stats["wall_s"] > 0 else None
            vram = backend.get_state_summary().get("vram_bytes")
            rows.append(BenchRow(batch=int(batch),
                                 sim_seconds_per_wall_second=per_wall,
                                 vram_bytes=vram, ok=True))
        except Exception as exc:  # OOM or other failure -> stop increasing
            rows.append(BenchRow(batch=int(batch),
                                 sim_seconds_per_wall_second=None,
                                 vram_bytes=None, ok=False,
                                 error=f"{type(exc).__name__}: {exc}"))
            break
    return rows


def choose_batch(rows: list[BenchRow], vram_headroom: float = 0.8) -> int:
    """Largest ok batch with the best throughput per individual.

    Throughput per individual = sim_seconds_per_wall_second / batch is
    roughly constant, so we take the largest ok batch (subject to the
    headroom note kept for future VRAM-aware refinement).
    """
    ok = [r for r in rows if r.ok]
    if not ok:
        return 1
    return max(r.batch for r in ok)
