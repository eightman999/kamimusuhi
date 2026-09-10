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
    --device cuda:0 --execution-batch auto --bench
```

Worker lifecycle: `GET /api/worker/profile` (the experiment's backend /
dataset / neuron count / duration / candidates / VRAM headroom) →
startup benchmark **on those conditions** (`--execution-batch auto` or
`--bench`) → register (device, GPU identity, runtime info, bench rows,
selected batch) → claim → evaluate → deliver → claim … Equivalent CLI:
`scripts/mioba bench --device cuda:0 --backend torch --synthetic-neurons N`.

### Scientific replicates vs. GPU execution batch

```yaml
evaluation:
  replicates: 32          # scientific: how many independent simulations score a genome
worker:
  execution_batch: auto   # operational: how many of them run at once on this GPU
  vram_headroom: 0.85     # reserved_vram / total_vram must stay below this
```

`evaluation.replicates` is part of `scientific_config_hash`; every job
carries it and its replicate seeds are
`blake2b(f"{evaluation_seed}:{index}")` — fixed by the job, not by the
worker. A worker with `execution_batch=16` runs 32 replicates as 16+16,
one with `execution_batch=4` as 4×8; both deliver the same 32 per-replicate
results and the same fitness (tested on CPU for batch 1/4/16, mock and
torch). GPU speed changes wall time and throughput only, never the sample
count, the random conditions or the selection. Each evaluation row stores
`requested_replicates`, `completed_replicates`, `execution_batch_size` and
`replicate_seeds_json`.

`choose_batch()` drops OOM/failed candidates and those above the VRAM
headroom, then takes the highest throughput (`sim_seconds_per_wall_second
× batch`) — not simply the largest batch. Bench rows (`vram_allocated_bytes`,
`vram_reserved_bytes`, `vram_total_bytes`, `sim_seconds_per_wall_second`,
`throughput`, `ok`, `error`) and the selection are stored in
`worker_runs.bench_json` / `batch_size`.

### Result delivery

Each result carries a deterministic `result_id =
blake2b(job_id:worker_id:attempt)`. The worker keeps `current_job_id`
set (`running` → `delivering`) and re-POSTs on connection errors / HTTP
5xx with exponential backoff until the coordinator answers 200 (accepted
or `duplicate` = already committed). A 409 (job reclaimed by another
worker, stale attempt) or 404 stops the retry. Only then is
`current_job_id` cleared and the next job claimed.

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
`duration_ms`, `backend`, `requested_replicates`, `completed_replicates`,
`execution_batch_size`, `replicate_seeds_json`, `result_id`, `device`,
`config_hash`, `scientific_config_hash`, `git_commit`, `runtime_json`
(device string, GPU index, GPU UUID, model, compute capability, VRAM,
driver, CUDA runtime, torch version — for the *selected* `--device`, not
GPU 0) and `dataset_json` — the *logical* dataset identity
(`dataset_id`, `version`, `manifest_hash`, `region_mode`), never a raw
path.

```bash
scripts/mioba --runs-dir .runs replay <evaluation-id>                 # recorded backend/dataset/seed/env/duration/replicates/seeds
scripts/mioba --runs-dir .runs replay <evaluation-id> --strict        # exit 5 on config/git/backend/GPU-model/compute-capability drift
scripts/mioba --runs-dir .runs replay <evaluation-id> --strict --allow-device-drift --device cuda:1   # explicit cross-GPU parity check
scripts/mioba --runs-dir .runs replay <evaluation-id> --execution-batch 1   # operational: replay the same replicates in another batch
scripts/mioba --runs-dir .runs replay <evaluation-id> --data-dir /data/flywire_783   # real-FBA recording
scripts/mioba --runs-dir .runs replay <evaluation-id> --backend genn   # cross-backend parity check (flagged as a warning)
```

Replay rebuilds the backend from the recorded dataset identity. A
real-FBA recording whose dataset is absent, or whose manifest hash
differs, exits 4 with `ReplayUnavailable: required dataset ... is not
available` — it is never silently re-run on a synthetic network. Legacy
rows without `dataset_json` are likewise unreplayable.

**Device identity.** Strict replay compares the recorded GPU model and
compute capability (and device type) with the replaying device: a 3060
(`8.6`) recording replayed on the P100 (`6.0`) fails unless
`--allow-device-drift` is given, in which case the drift is recorded in
the report as a parity check. GPU UUID / index / torch / CUDA-runtime
differences are informational warnings.

**Config hashes.** `config_hash` covers the whole config.
`scientific_config_hash` covers the result-affecting sections
(`population`, `evolution`, `evaluation`, `fba` minus `data_dir`, `env`,
`fitness`, `environment`, `dataset`); `runtime_config_hash` covers the
rest (GUI, heartbeat/checkpoint intervals, `worker.execution_batch`,
`worker.vram_headroom`, MIE, ...). On `--resume` a runtime change is
recorded (`runtime_config_changed`) and allowed; a scientific change
(backend, environment, mutation parameters, population, FBA parameters,
dataset, duration, fitness, **replicates**) raises
`ScientificConfigMismatch` and requires a **new experiment ID**. There is
no override flag.

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
| `smoke_a_b_backend.sh --device X --backend Y [--neurons N] [--edges E] [--out r.json]` | device recognition, synthetic init/run, artificial-organ run (asserts sparse layout), checkpoint→100 steps→restore→same 100 steps, same-seed determinism, execution_batch 1/2/4 replicate invariance |
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
# FlyWire scale: 139k neurons with an EXPLICIT 14M sampled edges (dataset id v0-n139000-e14000000).
# Without --edges the synthetic network uses connectivity p=0.01, i.e. 139000^2*0.01 ≈ 193M edges — not FlyWire-like.
bash experiments/mioba/scripts/smoke_a_b_backend.sh --device cuda:0 --backend torch --neurons 139000 --edges 14000000 --out gpu-validation/smoke_ab_cuda0_139k.json
bash experiments/mioba/scripts/smoke_a_b_backend.sh --device cuda:1 --backend torch --neurons 139000 --edges 14000000 --out gpu-validation/smoke_ab_cuda1_139k.json
```

The same explicit mode in a config: `fba: {synthetic: true,
synthetic_neurons: 139000, synthetic_edges: 14000000}` (edges are
sampled with replacement; duplicates coalesce and self-edges are dropped,
so `nnz` is marginally below the requested count).

Record for each: `device_recognized.info.capability` (`6.0` on P100,
`8.6` on 3060), `synthetic_init_run.info.vram_bytes`, and whether
`checkpoint_restore_replay` / `same_seed_deterministic` PASS on CUDA (if
they FAIL only on CUDA, that is the sparse-atomics nondeterminism noted
above — report the spike-count deltas), and
`execution_batch_invariance.info.identical_across_execution_batches`
(reported, not asserted, on CUDA for the same reason).

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
$MIOBA_PYTHON -m experiments.mioba.cli bench --backend torch --device cuda:0 --synthetic-neurons 139000 --synthetic-edges 14000000 --candidates 1,2,4,8,16,32 --duration-ms 500 --vram-headroom 0.85 --out gpu-validation/bench_cuda0_139k.json
$MIOBA_PYTHON -m experiments.mioba.cli bench --backend torch --device cuda:1 --synthetic-neurons 139000 --synthetic-edges 14000000 --candidates 1,2,4,8,16,32 --duration-ms 500 --vram-headroom 0.85 --out gpu-validation/bench_cuda1_139k.json
```

The JSON holds one row per candidate (`ok`, `sim_seconds_per_wall_second`,
`throughput`, `vram_allocated_bytes`, `vram_reserved_bytes`,
`vram_total_bytes`, `error` — an OOM stops the sweep), `selected_batch`
(highest throughput within the headroom), the dataset identity and the
runtime info for that device.

### 3. Worker smoke C/H: two asynchronous workers, worker loss, UNKNOWN → requeue, stale result

Terminal 1 (coordinator; bind 0.0.0.0 if the workers are on another host):

```bash
cd ~/kamimusuhi && export MIOBA_PYTHON=~/mioba-venv/bin/python
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/gpu_smoke.yaml --runs-dir gpu-validation/runs start --host 0.0.0.0 --port 8870
```

Terminals 2 and 3 (one per GPU; `--execution-batch auto --bench` fetches
`/api/worker/profile`, benchmarks on the experiment's conditions and
reports rows + selection to the coordinator):

```bash
$MIOBA_PYTHON -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8870 --backend torch --device cuda:0 --execution-batch auto --bench --worker-id rtx3060 --heartbeat-s 2
$MIOBA_PYTHON -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8870 --backend torch --device cuda:1 --execution-batch auto --bench --worker-id p100    --heartbeat-s 2
```

Terminal 4 — checks:

```bash
curl -s http://127.0.0.1:8870/api/workers | $MIOBA_PYTHON -m json.tool      # both online; device cuda:0/cuda:1, gpu_model, compute_capability, gpu_uuid, batch_size, current_job_id
# GPU identity must follow --device: the p100 worker's runtime_info/heartbeat must show Tesla P100 / cc 6.0 / its own UUID, temperature and VRAM — never GPU 0's.
# every evaluation must have requested_replicates == completed_replicates == evaluation.replicates (8 in gpu_smoke.yaml) on BOTH workers,
# while execution_batch_size may differ per GPU:
EXP=$(ls gpu-validation/runs | head -1)
$MIOBA_PYTHON -c "import sqlite3; [print(r) for r in sqlite3.connect('gpu-validation/runs/$EXP/lineage.sqlite').execute('select worker_id, requested_replicates, completed_replicates, execution_batch_size from evaluations')]"
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
  -d '{"worker_id":"p100","job_id":"<a SUCCEEDED job_id from /api/jobs>","status":"SUCCEEDED","result_id":"res_manual","evaluation":{"summary":{}}}'
# expect HTTP 409 and a "stale_result_rejected" event (a worker stops retrying on 409)
# idempotent re-delivery: re-POST an already-committed result (same job_id + result_id) -> HTTP 200 {"ok":true,"duplicate":true}, no new evaluation row

# restart the killed worker; it re-registers and claims requeued jobs
$MIOBA_PYTHON -m experiments.mioba.workers.worker --coordinator http://127.0.0.1:8870 --backend torch --device cuda:1 --execution-batch auto --worker-id p100 --heartbeat-s 2 &

# result delivery under coordinator restart: kill -9 the coordinator while a worker is mid-job, restart it with --resume;
# the worker keeps current_job_id in its heartbeat and retries the POST until 200 (or 409 if the job was requeued and reclaimed).
# Expect no duplicate evaluations for one job_id:
$MIOBA_PYTHON -c "import sqlite3; print(sqlite3.connect('gpu-validation/runs/$EXP/lineage.sqlite').execute('select job_id, count(*) from evaluations group by job_id having count(*)>1').fetchall())"   # expect []
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
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/gpu_smoke.yaml --runs-dir gpu-validation/runs --experiment-id "$EXP" replay "$EV" --device cuda:1 --strict
# cross-device strict replay (3060 recording on P100 or vice versa): must exit 5 with "GPU model differs ... compute capability differs".
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/gpu_smoke.yaml --runs-dir gpu-validation/runs --experiment-id "$EXP" replay "$EV" --device cuda:1 --strict --allow-device-drift
# explicit parity check: runs, records recorded_device/current_device; exact match is not expected.
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/gpu_smoke.yaml --runs-dir gpu-validation/runs --experiment-id "$EXP" replay "$EV" --device cuda:0 --execution-batch 1
# same GPU, different execution batch: identical_replicate_seeds must be true; identical_spike_counts true unless CUDA sparse atomics differ.
```

### 6. GUI (F) and MIE

Open `http://<tailscale-ip>:8870/` from the MacBook (UI is Japanese; times
shown in JST, stored in UTC): ダッシュボード shows both worker cards side by
side — `rtx3060 / NVIDIA GeForce RTX 3060 / cuda:0 / CC 8.6` and `p100 /
Tesla P100-PCIE-16GB / cuda:1 / CC 6.0` — with VRAM, 温度, GPU使用率,
実行バッチ, 実行中ジョブ, 完了/失敗ジョブ and the benchmark throughput
(sim-s/wall-s). Check the values differ per card (they come from each
worker's own GPU). Also 個体群・系譜, 個体インスペクター, MIE / テレメトリ,
イベント. Closing the tab must not affect `/api/status`; the GUI has no
controls.

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
  evaluation insert are one transaction; one generation — elites, parents,
  children, mutations, births, jobs, counters, post-mutation RNG state —
  is one transaction, tested with crashes at child 0/3/7); a worker crash
  mid-job leaves the job RUNNING → UNKNOWN → requeued, its partial work
  is lost.
- Execution-batch invariance is proven on CPU (mock + torch); on CUDA the
  same replicate seeds are used but sparse-atomics nondeterminism may
  still change spike counts between batch sizes — measure it (smoke A/B
  step 7).
- Schema v3 migrates v2 DBs (added columns only); v1 layouts of this PR
  are not migrated.

## Human checklist before a long run

1. Sections 0–6 above completed on both GPUs; JSON reports archived.
2. `/api/runtime` and `env-info` show `+cu126`, cc `6.0` (P100) / `8.6`
   (3060), the expected driver, and the git commit you intend to run.
3. Replay of a GPU evaluation with `--strict` exits 0; note whether
   `identical_spike_counts` is true per GPU.
4. `bench_*.json` `selected_batch` per GPU is what `worker.execution_batch`
   / `--execution-batch auto` will use; `vram_reserved_bytes /
   vram_total_bytes` of the selected row is below `worker.vram_headroom`
   at FlyWire scale.
5. Both workers online in the GUI with the right GPU model / CC per
   `--device`; killing one does not stall the other; every evaluation
   has `completed_replicates == evaluation.replicates` regardless of
   which GPU ran it.
5b. `evaluation.replicates` is the value you want for the whole run — it
   is scientific and cannot be changed under the same experiment ID.
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
