# MIOBA architecture

## Organism model (M2)

M1 built the organism as *FBA0 plus artificial organs*: the FlyWire
connectome was the architecture's implicit body. M2 makes the organism
generic:

```
organism
 ├─ substrate(s)      SubstrateGene list → substrate registry adapters
 ├─ organs            organ IR: kind, size, params, typed ports,
 │                    internal topology, runtime-state schema
 ├─ transducers       sensor:* endpoints (habitat -> event)
 ├─ internal state    organ state schema + LifetimeState (never in the
 │                    genome)
 └─ effectors         effector:* endpoints (event -> habitat)
```

FBA0 is the M-series founder condition (`substrate/fba0.py` behind
`substrate/base.py`'s `SubstrateProtocol`); every M1 genome implicitly
carries it, and schema v4 spells it out explicitly. M3 added a second
registered substrate — `substrate/reflex0.py`, a 48-neuron
sensor→integrator→motor arc with typed ports — purely to prove the
abstraction is real: a non-FBA genome develops, mutates, lesions,
evaluates, reproduces and replays through exactly the same machinery,
with no reflex0 branch anywhere in the generic layer (the tripwire
lives in `tests/test_m3_reflex0.py`). Nothing in the
architecture-level code imports the FBA0 implementation:
`development/phenotype.py` resolves substrate genes through
`substrate/registry.py`, and `genome/structure.py` classifies
endpoints through the typed `substrate/endpoints.py` parser.

Two topology modes exist in `analyse()`: `m1_fba0_loop` (the frozen
historical FBA0→organ→FBA0 rule) and `generic_causal` (sources =
enabled substrates + sensor/env, sinks = enabled substrates +
effector/env). Every M1 genome classifies identically under both.

The genome is the recipe, not the body: `development/rules.py` applies
deterministic birth-stage `development_rules` (ADD/GROW/SCALE organ at
birth), and lifetime-stage ops are deferred to
`development/lifetime.py`'s `LifetimeState` — per-individual runtime
state that is never hashed and never inherited. The mutation/
plasticity boundary: a child inherits the *rules*, not the parent's
*adjustments*.

Functional departure (`m2/departure.py`, `functional_departure.enabled`
in config) measures what an individual actually uses: intact, sham,
founder, organ-ablation and per-severity substrate-lesion conditions on
the same backend under the same seeds, with lesion masks from the
substrate adapter under a dedicated `lesion` seed stream. Raw condition
scores persist under `metrics.departure`; `departure_resistance` is a
selection component (default weight 0). Control lineages exist so that
"structure changed" is separable from "selected for departure":
`m2_control_parameter_only.yaml` restricts the mutation pool to
`SCALE_PARAMETER`, and `m2_control_structural_neutral.yaml` runs the
battery with selection weight 0.

M3 scaffolding exists but is inert: `SUBSTRATE_OPERATORS`
(DISABLE/BYPASS/PRUNE/REPLACE_SUBSTRATE_REGION) are applicable via
`apply_operator` yet absent from every selection pool. What M3 *did*
harden: a native-v4 genome that declares substrate genes but disables
every one is substrate-less — `develop()` raises `NoEnabledSubstrate`
rather than substituting the implicit FBA0 (only the *absent* field
means the founder), genome-disabled substrate ports are dangling at
both develop() and structure analysis, and the worker reports
development failures as FAILED evaluations instead of crash-looping.

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
