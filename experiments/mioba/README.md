# MIOBA experiments

Infrastructure for the MIOBA M-series experiments: an evolutionary loop over
artificial organisms built on the FBA0 reference (the FlyWire v783
connectome simulated with the Shiu et al. 2024 LIF + delayed alpha-synapse
model), coordinated by a FastAPI service with a SQLite lineage database,
GPU workers, machine-interoception (MIE) telemetry, and a read-only
Observatory GUI.

This package is *infrastructure only* — phase 1 ships the coordinator,
storage, backends, worker protocol, MIE collectors, E-adapter contract,
checkpoint/resume and smoke tests. It is not validated for long evolution
runs and has not been run on GPU hardware.

## Install

```bash
python3.10 -m venv ~/mioba-venv
~/mioba-venv/bin/pip install -r experiments/mioba/requirements.txt   # dev/CPU box
# GPU host (RTX 3060 / P100, sm_60 kept by cu126 wheels):
~/mioba-venv/bin/pip install -r experiments/mioba/requirements-gpu-cu126.txt
```

`requirements-gpu-cu126.txt` pins `torch==2.9.1+cu126` because cu128/cu129 wheels
dropped sm_60 (Pascal/P100); CUDA 13 is not required. PyGeNN
(`pygenn==5.4.0`, needs a CUDA 12.x toolkit build) is optional — the
`genn` backend reports itself unavailable without it and is otherwise
unverified on non-GPU hosts.

## Quickstart

```bash
scripts/mioba --config experiments/mioba/configs/smoke_mock.yaml \
    --runs-dir .runs start --port 8870          # foreground coordinator
scripts/mioba --runs-dir .runs status           # one-line status (--json for full)
scripts/mioba --runs-dir .runs pause            # stop handing out claims
scripts/mioba --runs-dir .runs resume           # resume claims (live coordinator)
scripts/mioba --runs-dir .runs checkpoint       # WAL checkpoint + manifest
scripts/mioba --runs-dir .runs stop --timeout 60
```

Runtime state lives under `<runs-dir>/<experiment_id>/`:
`lineage.sqlite`, `checkpoints/`, `logs/`, `telemetry/`, `traces/`,
`coordinator.json`, `control.token`.

## Workers

On each GPU host:

```bash
python -m experiments.mioba.workers.worker \
    --coordinator http://<tailscale-ip>:8870 --backend torch \
    --device cuda:0 --batch auto --bench
```

`--batch auto` (or `--bench`) runs the startup benchmark over
`worker.candidates` and reports the rows to the coordinator
(`worker_runs.bench_json`). Equivalent CLI: `scripts/mioba bench
--device cuda:0 --backend torch`.

## GUI (Observatory)

`http://<tailscale-ip>:8870/` (or bind `--host 0.0.0.0`). The GUI is
read-only: control endpoints (`pause`/`resume`/`checkpoint`/`stop`)
require the `X-Mioba-Token` header, whose value is written to
`<run_dir>/control.token` at start and read by the CLI automatically.
Closing or losing the GUI never affects the experiment — the coordinator
and its DB are the only source of truth.

## Checkpoint & resume

Checkpoints = `PRAGMA wal_checkpoint(TRUNCATE)` + a manifest JSON under
`<run_dir>/checkpoints/` + a `checkpoints` row. The DB itself is durable.

- Live coordinator: `mioba checkpoint`; `mioba pause` / `mioba resume`.
- Dead coordinator (crash / `kill -9`): restart with the same runs dir —
  `mioba start --resume <experiment_id> --runs-dir ...`. RUNNING jobs are
  marked UNKNOWN then requeued per `jobs.requeue_unknown`; workers
  re-register. If the config changed since the run started, a
  `config_hash_mismatch` warning event is emitted and the stored hash is
  exposed in `/api/status` as `config_hash_stored`.

## Smoke tests

`scripts/smoke_all.sh <outdir>` runs A–H sequentially and writes
`smoke-results.txt`. Individually:

| script | covers |
|---|---|
| `smoke_a_b_backend.sh --device X --backend Y` | backend init/run/summary |
| `smoke_c_two_workers.sh` | two workers claim distinct jobs |
| `smoke_d_genome_eval.sh` | genome → evaluation row |
| `smoke_e_resume.sh` | checkpoint, SIGKILL, resume |
| `smoke_f_gui.sh` | GUI endpoints on a live run |
| `smoke_g_pause_stop.sh` | pause blocks claims; stop → stopped + checkpoint |
| `smoke_h_worker_loss.sh` | lost worker → job UNKNOWN → requeue |

`bench_gpu.sh` = the GPU-side A/B (run on the GPU host).

## Known limitations

- `torch` backend correctness beyond the smoke path and the `genn`
  backend are unverified until run on a GPU host with real data.
- `fitness` is a placeholder (`-|mean_rate_hz - target|`).
- `synthetic=True` connectivity is random sparse, not the connectome.
- Full spike traces are only stored when `requested_traces` is non-empty.

## Human checklist before a long run

1. GPU hosts pass `smoke_a_b_backend.sh --device cuda:0 --backend torch`.
2. `/api/runtime` shows the expected GPU (`sm_60` capability on P100,
   torch `+cu126`).
3. All workers appear online in the GUI (`/api/gui/dashboard`).
4. `checkpoint.interval_s` and `stop.timeout_s` are sane for the run.
5. Enough disk space under `runs_dir`; `runs_dir` is on persistent disk
   (not tmpfs).
6. Tailscale ACL allows the worker hosts to reach the coordinator port.
7. Record the `config_hash` (in `/api/status`) for the run.
8. Decide `evolution.max_generations` / `population.target_size`.
9. Take a DB backup of `lineage.sqlite` before starting.

## Upstream & licensing

- `eonsystemspbc/fly-brain` (GPL-2.0-or-later): equations and data-file
  layout were read for reference only; **no code was copied or
  vendored**. The `2025_Connectivity_783.parquet` /
  `2025_Completeness_783.csv` data files are used read-only from a
  caller-provided `MIOBA_FLY_BRAIN_DATA` directory.
- FlyWire v783 connectivity data; LIF/alpha-synapse model parameters from
  Shiu et al. 2024.
- GeNN/PyGeNN (optional backend), PyTorch, FastAPI/uvicorn, httpx,
  psutil, PyYAML, numpy.
- Genome hashing uses `hashlib.blake2b` (stdlib) with a `b2b:` prefix
  reserved for a future BLAKE3 migration; there is no `blake3` package
  dependency.
