# MIOBA architecture (phase 1)

## Components

```
            ┌─────────────────────────────────────────────────────┐
            │ coordinator (FastAPI + uvicorn, single writer)       │
            │  app.py        HTTP API + worker protocol            │
            │  service.py    MiobaService: DB, RNG, bg loop (1 Hz) │
            │  lifecycle.py  pause/resume/checkpoint/stop/resume   │
            │  jobs.py       claim/finish/requeue policy           │
            └──────┬──────────────────┬──────────────────┬────────┘
                   │                  │                  │
   POST /api/worker/*          SQLite (WAL)        mount_gui()
   register/heartbeat/    lineage.sqlite +        / + /api/gui/*
   claim/result           checkpoints/,           static SPA
                   │      telemetry/, traces/          │
            ┌──────┴──────┐                            │
            │  workers    │                    Observatory GUI
            │ worker.py   │                    (read-only SPA)
            │ bench.py    │
            │ gpu_info.py │
            └──────┬──────┘
                   │ develops genome → FBA backend
            ┌──────┴───────────────────────────┐
            │ fba/: mock | torch | genn        │
            │ (Shiu 2024 LIF + alpha synapse,  │
            │  1.8 ms delay ring buffer)       │
            └──────────────────────────────────┘

   MIE collectors (coordinator-side + worker heartbeats):
   gpu/cpu/ram/network_latency → telemetry_samples + samples.jsonl
```

- **Coordinator** is the only writer to `lineage.sqlite`. The GUI and CLI
  never own state.
- **Workers** are stateless executors: register → (bench) → claim →
  develop → backend.initialize → run → result. Heartbeats carry GPU
  stats and MIE samples. A worker never marks anything succeeded on its
  own; it only reports.
- **MIE** collectors produce `SensorEvent`s, normalized/delta'ed, stored
  in `telemetry_samples`; temperature above `mie.anomaly.gpu_temp_c`
  raises `sensor_anomaly` events.

## LIVE / RECORDED / DERIVED

Every API payload carries `kind`:

- `LIVE` — current runtime state (status, workers, runtime info).
- `RECORDED` — rows read from the DB (genomes, evaluations, events,
  telemetry).
- `DERIVED` — computed on request from recorded rows (throughput,
  ancestry chains, phenotype summaries, latest-per-signal telemetry).

## Job state machine

```
QUEUED ──claim (atomic UPDATE ... WHERE status='QUEUED')──▶ RUNNING
RUNNING ──worker result──▶ SUCCEEDED | FAILED
RUNNING ──stale worker (> worker.lost_after_s) or coordinator restart──▶ UNKNOWN
UNKNOWN ──jobs.requeue_unknown──▶ QUEUED (attempt+1)  | FAILED at max_attempts
any non-terminal ──cancel──▶ CANCELLED
```

- `RUNNING → SUCCEEDED` is never inferred; only an explicit worker result
  finishes a job, and only from the worker currently holding the claim —
  a late result from a worker that lost its claim (UNKNOWN → requeued →
  re-claimed by another worker) is rejected with `stale_result_rejected`
  (warn) and HTTP 409.
- `pause` stops claims only (claim returns 204); `stop` sets
  `stopping`, waits for RUNNING jobs up to `stop.timeout_s`, marks the
  rest UNKNOWN, writes a `reason=stop` checkpoint, WAL-truncates, exits.

## Checkpoint manifest

`<runs>/<exp>/checkpoints/<ts>-<reason>.json`:
`experiment_id, created_at, reason, git_commit, config_hash, counters,
rng_state, queue_counts, worker_ids, db_sha256, last_event_id` — plus a
`checkpoints` row.

## Resume rules

`mioba start --resume <experiment_id>` reopens the existing DB (a runs
dir that already holds an experiment refuses a fresh start without
`--resume`). On resume: RUNNING → UNKNOWN (`job_marked_unknown`,
reason `coordinator_restart`) then requeue per policy; online workers are
marked offline and must re-register; `experiment_resumed_from_checkpoint`
is emitted. If the config changed since the run started, a
`config_hash_mismatch` warn event is emitted (the run continues — the
operator may have legitimately changed intervals) and `/api/status`
exposes both `config_hash` and `config_hash_stored`.

## Boundaries

- **vs Kamimusuhi canonical identity**: MIOBA genome lineage is
  experiment data in `lineage.sqlite`; it is *not* canonical identity
  lineage and never touches kamimusuhi crates or stores. Symbols are
  named `genome_lineage`, never `continuity`.
- **vs fly-brain**: the FBA0 reference model's equations and data-file
  layout were read for reference only; fly-brain is GPL-2.0-or-later and
  no code is copied or vendored. Data files are read from a
  caller-provided `MIOBA_FLY_BRAIN_DATA` directory at runtime.
