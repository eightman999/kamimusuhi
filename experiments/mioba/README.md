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

## Reproducibility & replay

Every evaluation row stores: `genome_hash`, `seed`, `environment_id`,
`duration_ms`, `backend`, `batch_size`, `device`, `config_hash`,
`scientific_config_hash`, `git_commit`, `runtime_json` (GPU/driver/CUDA/
torch versions) and `dataset_json` — the *logical* dataset identity
(`dataset_id`, `version`, `manifest_hash`, `region_mode`), never a raw
path.

```bash
scripts/mioba --runs-dir .runs replay <evaluation-id>                 # recorded backend/dataset/seed/env/duration/batch
scripts/mioba --runs-dir .runs replay <evaluation-id> --strict        # exit 5 on any config/git/backend/device drift
scripts/mioba --runs-dir .runs replay <evaluation-id> --data-dir /data/flywire_783   # real-FBA recording
scripts/mioba --runs-dir .runs replay <evaluation-id> --backend genn   # cross-backend parity check (flagged as a warning)
```

Replay rebuilds the backend from the recorded dataset identity. A
real-FBA recording whose dataset is absent, or whose manifest hash
differs, exits 4 with `ReplayUnavailable: required dataset ... is not
available` — it is never silently re-run on a synthetic network. Legacy
rows without `dataset_json` are likewise unreplayable.

**Config hashes.** `config_hash` covers the whole config.
`scientific_config_hash` covers the result-affecting sections
(`population`, `evolution`, `evaluation`, `fba` minus `data_dir`, `env`,
`fitness`, `environment`, `dataset`); `runtime_config_hash` covers the
rest (GUI, heartbeat/checkpoint intervals, MIE, ...). On `--resume` a
runtime change is recorded (`runtime_config_changed`) and allowed; a
scientific change raises `ScientificConfigMismatch` unless
`--allow-scientific-change` is passed (then recorded as
`scientific_config_mismatch`).

**Determinism (what is and isn't guaranteed).** Mutation, organ and
attachment IDs derive from the mutation `random.Random`, so the same
parent + RNG state + generation + birth index gives the same child
`genome_id`. The torch backend draws every random number from explicit
`torch.Generator`s whose state (plus global torch / CUDA RNG state) is
part of `checkpoint()`; on CPU, checkpoint → run 100 steps → restore →
same 100 steps is bit-identical (tested). On CUDA, `torch.sparse.mm`
uses atomics and may not be bit-reproducible run-to-run; this is a
backend limitation to be measured on the GPU host (below), not a
guarantee.

## Smoke tests (CPU / mock — run in CI)

`scripts/smoke_all.sh <outdir>` runs A–H sequentially and writes
`smoke-results.txt`. Individually (`MIOBA_PYTHON=python` selects the
interpreter):

| script | covers |
|---|---|
| `smoke_a_b_backend.sh --device X --backend Y [--neurons N] [--out r.json]` | device recognition, synthetic init/run, artificial-organ run (asserts sparse layout), checkpoint→100 steps→restore→same 100 steps, same-seed determinism |
| `smoke_c_two_workers.sh` | two workers claim distinct jobs |
| `smoke_d_genome_eval.sh` | genome → evaluation row |
| `smoke_e_resume.sh` | checkpoint, SIGKILL, resume |
| `smoke_f_gui.sh` | GUI endpoints on a live run |
| `smoke_g_pause_stop.sh` | pause blocks claims; stop → stopped + checkpoint |
| `smoke_h_worker_loss.sh` | lost worker → job UNKNOWN → requeue |

GitHub Actions `MIOBA Python CI` runs `pytest experiments/mioba/tests`,
import/compile and CLI checks, and smokes A/B (cpu), C, D, E, G, H on a
CPU runner. GPU behaviour is **not** covered by CI.

## Human GPU validation (Tesla P100 16GB + RTX 3060 12GB, CUDA 12.6)

Nothing below has been run by the authors on real GPUs; the CPU-only
development environment cannot validate P100/3060 behaviour. Run these on
the GPU host(s) and keep the JSON outputs with the PR/run. Assumes the
repo is checked out at `~/kamimusuhi` and the venv at `~/mioba-venv`
(from *Install*, `requirements-gpu-cu126.txt`). `cuda:0`/`cuda:1` below
are placeholders — check `nvidia-smi -L` for which index is which card.

### 0. Runtime

```bash
cd ~/kamimusuhi && export MIOBA_PYTHON=~/mioba-venv/bin/python
nvidia-smi -L; nvidia-smi --query-gpu=name,driver_version,compute_cap,memory.total --format=csv
$MIOBA_PYTHON -c "import torch; print(torch.__version__, torch.version.cuda, torch.cuda.is_available(), [torch.cuda.get_device_name(i) for i in range(torch.cuda.device_count())])"
# expected: 2.9.1+cu126  12.6  True  ['Tesla P100-PCIE-16GB', 'NVIDIA GeForce RTX 3060'] (order may differ)
$MIOBA_PYTHON -m experiments.mioba.cli env-info          # GPU model / driver / CUDA / cc / torch / git commit
```

### 1. Backend smoke A (3060) / B (P100)

Each covers torch import, CUDA device recognition, synthetic FBA
initialize + run, artificial-organ run on the sparse path, checkpoint →
100 steps → restore → replay, and same-seed determinism.

```bash
mkdir -p gpu-validation
bash experiments/mioba/scripts/smoke_a_b_backend.sh --device cuda:0 --backend torch --neurons 20000  --out gpu-validation/smoke_ab_cuda0_20k.json
bash experiments/mioba/scripts/smoke_a_b_backend.sh --device cuda:1 --backend torch --neurons 20000  --out gpu-validation/smoke_ab_cuda1_20k.json
# FlyWire scale (~139k neurons, ~14M synthetic edges) — this is the real memory test for the sparse path:
bash experiments/mioba/scripts/smoke_a_b_backend.sh --device cuda:0 --backend torch --neurons 139000 --out gpu-validation/smoke_ab_cuda0_139k.json
bash experiments/mioba/scripts/smoke_a_b_backend.sh --device cuda:1 --backend torch --neurons 139000 --out gpu-validation/smoke_ab_cuda1_139k.json
```

Record for each: `device_recognized.info.capability` (`6.0` on P100,
`8.6` on 3060), `synthetic_init_run.info.vram_bytes`, and whether
`checkpoint_restore_replay` / `same_seed_deterministic` PASS on CUDA (if
they FAIL only on CUDA, that is the sparse-atomics nondeterminism noted
above — report the spike-count deltas).

Real dataset (only if the FlyWire v783 files are present):

```bash
export MIOBA_FLY_BRAIN_DATA=/data/flywire_783    # 2025_Connectivity_783.parquet + 2025_Completeness_783.csv
$MIOBA_PYTHON - <<'EOF'
from experiments.mioba.development.phenotype import develop
from experiments.mioba.fba.registry import get_backend
from experiments.mioba.genome.schema import fba0_genome
b = get_backend("torch", synthetic=False)
b.initialize(develop(fba0_genome()), batch_size=1, seed=0, device="cuda:0")
print(b.dataset_identity(), b.n, b.nnz, b.get_state_summary()["vram_bytes"])
print(b.run(100))
EOF
```

### 2. Benchmark per GPU (batch candidates, sim-s/wall-s, VRAM, OOM, selected batch)

```bash
$MIOBA_PYTHON -m experiments.mioba.cli bench --backend torch --device cuda:0 --synthetic-neurons 20000  --candidates 1,2,4,8,16,32 --duration-ms 500 --out gpu-validation/bench_cuda0_20k.json
$MIOBA_PYTHON -m experiments.mioba.cli bench --backend torch --device cuda:1 --synthetic-neurons 20000  --candidates 1,2,4,8,16,32 --duration-ms 500 --out gpu-validation/bench_cuda1_20k.json
$MIOBA_PYTHON -m experiments.mioba.cli bench --backend torch --device cuda:0 --synthetic-neurons 139000 --candidates 1,2,4,8,16,32 --duration-ms 500 --out gpu-validation/bench_cuda0_139k.json
$MIOBA_PYTHON -m experiments.mioba.cli bench --backend torch --device cuda:1 --synthetic-neurons 139000 --candidates 1,2,4,8,16,32 --duration-ms 500 --out gpu-validation/bench_cuda1_139k.json
```

The JSON holds one row per candidate (`ok`, `sim_seconds_per_wall_second`,
`vram_bytes`, `error` — an OOM stops the sweep), `selected_batch`, the
dataset identity and the runtime info.

### 3. Worker smoke C/H: two asynchronous workers, worker loss, UNKNOWN → requeue, stale result

Terminal 1 (coordinator; bind 0.0.0.0 if the workers are on another host):

```bash
cd ~/kamimusuhi && export MIOBA_PYTHON=~/mioba-venv/bin/python
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/gpu_smoke.yaml --runs-dir gpu-validation/runs start --host 0.0.0.0 --port 8870
```

Terminals 2 and 3 (one per GPU; `--batch auto --bench` runs the startup
benchmark and reports it to the coordinator):

```bash
$MIOBA_PYTHON -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8870 --backend torch --device cuda:0 --batch auto --bench --worker-id rtx3060 --heartbeat-s 2
$MIOBA_PYTHON -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8870 --backend torch --device cuda:1 --batch auto --bench --worker-id p100    --heartbeat-s 2
```

Terminal 4 — checks:

```bash
curl -s http://127.0.0.1:8870/api/workers | $MIOBA_PYTHON -m json.tool      # both online, gpu info, batch, current job
curl -s http://127.0.0.1:8870/api/status                                     # evaluations_succeeded rises; queued drains
curl -s "http://127.0.0.1:8870/api/jobs?limit=50" | $MIOBA_PYTHON -c "import json,sys; [print(j['job_id'][:8], j['status'], j['worker_id']) for j in json.load(sys.stdin)['jobs']]"
# fast worker must not wait for the slow one: the 3060 should complete more jobs than the P100 in the same wall time
# (compare completed_jobs per worker in /api/workers while the queue is non-empty).

# worker loss -> UNKNOWN -> requeue: kill one worker mid-job
pkill -9 -f "worker-id p100"
sleep 25                                          # > worker.lost_after_s (20)
curl -s "http://127.0.0.1:8870/api/events?limit=30" | $MIOBA_PYTHON -c "import json,sys,collections; print(collections.Counter(e['type'] for e in json.load(sys.stdin)['events']))"   # expect worker_lost, job_marked_unknown, job_requeued
curl -s http://127.0.0.1:8870/api/status          # unknown -> 0 after requeue; the other worker keeps completing
# stale result rejection: a result for a job that is no longer RUNNING/owned by the sender is rejected
curl -s -X POST http://127.0.0.1:8870/api/worker/result -H 'content-type: application/json' \
  -d '{"worker_id":"p100","job_id":"<a SUCCEEDED job_id from /api/jobs>","status":"SUCCEEDED","evaluation":{"summary":{}}}'
# expect HTTP 409 and a "stale_result_rejected" event

# restart the killed worker; it re-registers and claims requeued jobs
$MIOBA_PYTHON -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8870 --backend torch --device cuda:1 --batch auto --worker-id p100 --heartbeat-s 2 &
```

### 4. Checkpoint / kill / resume (E) and pause / graceful stop (G) with GPU workers

```bash
$MIOBA_PYTHON -m experiments.mioba.cli --runs-dir gpu-validation/runs checkpoint
kill -9 $(pgrep -f "experiments.mioba.cli .*start")           # coordinator dies
EXP=$(ls gpu-validation/runs | head -1)
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/gpu_smoke.yaml --runs-dir gpu-validation/runs start --resume "$EXP" --host 0.0.0.0 --port 8870 &
sleep 5; curl -s http://127.0.0.1:8870/api/status              # status running, genomes/evaluations preserved, RUNNING -> UNKNOWN -> requeued
$MIOBA_PYTHON -m experiments.mioba.cli --runs-dir gpu-validation/runs pause     # workers stop receiving claims
$MIOBA_PYTHON -m experiments.mioba.cli --runs-dir gpu-validation/runs resume
$MIOBA_PYTHON -m experiments.mioba.cli --runs-dir gpu-validation/runs stop --timeout 60   # no new claims -> running jobs finish -> checkpoint -> stopped
```

### 5. Deterministic replay of a GPU evaluation

```bash
EXP=$(ls gpu-validation/runs | head -1)
EV=$($MIOBA_PYTHON -c "import sqlite3; print(sqlite3.connect('gpu-validation/runs/$EXP/lineage.sqlite').execute('select evaluation_id from evaluations order by created_at limit 1').fetchone()[0])")
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/gpu_smoke.yaml --runs-dir gpu-validation/runs --experiment-id "$EXP" replay "$EV" --device cuda:0 --strict
# diff.identical_spike_counts: true = bit-identical replay on this GPU. false = CUDA sparse nondeterminism; record the delta.
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/gpu_smoke.yaml --runs-dir gpu-validation/runs --experiment-id "$EXP" replay "$EV" --device cuda:1
# cross-device replay (3060 recording on P100 or vice versa): expect a "device type differs" warning; exact match is not expected.
```

### 6. GUI (F) and MIE

Open `http://<tailscale-ip>:8870/` from the MacBook: Dashboard shows both
workers with temperature/utilization/VRAM (from the worker heartbeats),
Population/Lineage, Organism inspector, MIE/Telemetry, Events. Closing
the tab must not affect `/api/status`.

## Known limitations

- **No GPU validation by the authors.** Everything CUDA-related (cu126
  wheels on sm_60, VRAM at FlyWire scale, CUDA determinism, two-GPU
  throughput) is pending the section above.
- CUDA `torch.sparse.mm` uses atomics; bit-exact replay on GPU is not
  guaranteed even with identical RNG state. CPU replay is bit-exact.
- The `genn` backend is a guarded adapter; unverified without PyGeNN.
- Real FBA0 data has no region→neuron mapping yet: `fba0:<region>`
  attachments raise `UnsupportedAttachmentRegion` on real data. On
  synthetic data they use the explicit `synthetic-region-v0` partition
  (recorded in `dataset_json.region_mode`) — this is *not* FlyWire
  anatomy.
- `fitness` is a placeholder (`-|mean_rate_hz - target|`).
- Full spike traces are only stored when `requested_traces` is non-empty.
- Crash safety covers the coordinator's SQLite writes (job success +
  evaluation insert are one transaction); a worker crash mid-job leaves
  the job RUNNING → UNKNOWN → requeued, its partial work is lost.
- Schema v2 DBs are not migrated from the earlier v1 layout of this PR.

## Human checklist before a long run

1. Sections 0–6 above completed on both GPUs; JSON reports archived.
2. `/api/runtime` and `env-info` show `+cu126`, cc `6.0` (P100) / `8.6`
   (3060), the expected driver, and the git commit you intend to run.
3. Replay of a GPU evaluation with `--strict` exits 0; note whether
   `identical_spike_counts` is true per GPU.
4. `bench_*.json` `selected_batch` per GPU is what `worker.batch_size`
   / `--batch` will use; VRAM headroom is acceptable at FlyWire scale.
5. Both workers online in the GUI; killing one does not stall the other.
6. `checkpoint.interval_s`, `stop.timeout_s`, `worker.lost_after_s` are
   sane for the run; `runs_dir` is on persistent disk with enough space.
7. Tailscale ACL allows the worker hosts to reach the coordinator port.
8. Record `config_hash` and `scientific_config_hash` from `/api/status`;
   any later scientific change is a *new* experiment ID.
9. Decide `evolution.max_generations` / `population.target_size`; take a
   backup of `lineage.sqlite` before starting.

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
