# MIOBA startup benchmark results

Method: `workers/bench.py::startup_benchmark` — for each candidate batch
size: initialize backend, run `duration_ms` of simulation, measure
simulated-seconds per wall-second (`sim_s/wall_s`).

Local box: x86_64, 8 CPUs, no GPU. All numbers below are **CPU/mock
numbers only** — nothing here validates GPU throughput.

## mock backend (N=512 sparse LIF, dt=0.1 ms, 200 ms sim)

| batch | sim_s/wall_s | vram_bytes | ok |
|---|---|---|---|
| 1 | 3.19 | — | yes |
| 2 | 2.37 | — | yes |
| 4 | 1.41 | — | yes |
| 8 | 1.18 | — | yes |

## torch backend, CPU (synthetic N=500 sparse, dt=0.1 ms, 200 ms sim)

| batch | sim_s/wall_s | vram_bytes | ok |
|---|---|---|---|
| 1 | 0.30 | — | yes |
| 2 | 0.32 | — | yes |
| 4 | 0.27 | — | yes |
| 8 | 0.28 | — | yes |

## GPU hosts

| host | GPU | backend | batch | sim_s/wall_s | vram_bytes | ok |
|---|---|---|---|---|---|---|
| RTX 3060 | — | torch | — | NOT MEASURED — run `scripts/bench_gpu.sh` on host | — | — |
| P100 (sm_60) | — | torch | — | NOT MEASURED — run `scripts/bench_gpu.sh` on host | — | — |
| P100 (sm_60) | — | genn | — | NOT MEASURED — pygenn build required, backend unverified | — | — |

Notes: `choose_batch` picks the largest ok candidate (throughput per
individual is roughly constant across batch sizes on CPU; on GPU, larger
batches should amortise kernel launch overhead). A failed candidate stops
the sweep and is recorded with its error.
