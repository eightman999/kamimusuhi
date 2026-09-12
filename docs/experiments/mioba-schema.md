# MIOA experimental schema (legacy `mioba` implementation)

This document describes the schema currently implemented under
`experiments/mioba/`. The implementation name **MIOBA** is retained for
reproducibility, but the organism-level architecture is now called
**MIOA — Machine Information Organism Architecture**.

The current schema represents the **ancestral neural-substrate phase** of MIOA:
an immutable FBA0 reference plus artificial organs, attachments and parameter
mutations. It does not yet claim to represent the complete future organism
model (sensory organs, effectors, habitat interfaces or physical replacement of
FBA0).

## Terminology and compatibility

- **MIOA**: whole organism architecture.
- **MIO**: one organism / individual.
- **FBA0**: current ancestral neural substrate, not the organism itself.
- **MIOBA**: historical package / CLI / environment-variable / experiment name.
- Existing paths, DB fields, experiment IDs and recorded M0/M1 artifacts are
  not renamed in place.

## Genome JSON (`schema_version = 1`)

| field | type | meaning |
|---|---|---|
| `genome_id` | str | content hash `b2b:<blake2b-32>`, filled by `finalize()` |
| `schema_version` | int | currently 1 |
| `parent_ids` | list[str] | parent genome_ids (genome lineage, not canonical identity) |
| `species_base` | str | currently `"fba0"`; founder substrate identifier |
| `generation` / `birth_index` | int | evolutionary position |
| `random_seed` | int | per-genome seed used by evaluation jobs |
| `ancestral_base` | str | FBA0 reference name (`flywire-v783-shiu-lif`) |
| `artificial_organs` | list[ArtificialOrgan] | current neural/artificial organ representation: `{organ_id, kind, size, params, provenance}` |
| `attachments` | list[Attachment] | `{attachment_id, source, target, weight_scale, provenance}`; source `"fba0:<region>"` or an organ_id |
| `parameter_mutations` | list[ParameterMutation] | `{mutation_id, path, op(set|scale|add), value, scope}`; scope `global`/`region:<x>`/`organ:<id>` |
| `development_rules` / `plasticity_rules` | list[dict] | reserved |
| `created_at` | str | ISO8601 UTC |

`OrganProvenance`: `{origin: "fba0"|"artificial", parent_gene,
birth_mutation_id, ancestry}`.

### What schema v1 means

Schema v1 should be read as:

```text
MIOA individual (current experimental subset)
├─ immutable ancestral substrate: FBA0
├─ artificial neural organs
├─ attachments
└─ parameter mutations
```

It should **not** be interpreted as a permanent statement that every MIOA must
have FBA0 or a single brain-like center. `species_base="fba0"` records the
founder condition of the present M-series.

### Future organ generalization

A later schema revision may generalize `artificial_organs` into explicit organ
roles such as:

- sensory / transducer organs;
- processing / recurrent organs;
- memory / persistent-state organs;
- machine-interoceptive organs;
- action / effector organs;
- communication / other-organism interfaces.

This is a design direction, **not implemented schema**. Schema v1 must not be
silently reinterpreted or extended without a schema-version change.

For sensory organs, architecture-level semantics should stay general:

```text
information source → transducer → organ encoding/dynamics → attachments
```

A camera-backed spatial organ and a microphone-backed resonance organ are early
possible instances. Human labels such as “eye” and “ear” are presentation or
phenotype descriptions, not privileged schema primitives. Machine-native
sources such as network timing, filesystem events or compute state can use the
same organ abstraction in later experiments.

### Habitat boundary

The planned display-inside sandbox is an **environment/habitat**, not a hidden
extension of the genome. A genome may encode how an organ consumes an explicit
habitat channel, but the experiment must separately record which external
source that channel was connected to.

Scientific inputs must therefore remain distinguishable from operational host
telemetry. A GPU temperature seen by the Observatory is not automatically an
organism sense; exposing it to the organism requires an explicit experiment
interface and provenance/trace path.

### Content hash rule

`genome_hash(genome)` = `"b2b:" + blake2b(canonical_json(genome without
genome_id, created_at), digest_size=32)`. Canonical JSON = sorted keys,
compact separators, `ensure_ascii`. `hashlib.blake2b` (stdlib) is used so
the hash is deterministic across processes; the `b2b:` prefix is a
version tag allowing a later BLAKE3 (`b3:`) migration without ambiguity.

The MIOA terminology change does not alter this hash rule and must not change
existing genome IDs.

## SQLite tables (`lineage.sqlite`, WAL, foreign keys ON)

All tables carry `experiment_id` unless noted. `schema_version(version)`
records the layout version.

| table | purpose / columns |
|---|---|
| `experiments` | one row per experiment: `experiment_id PK, created_at, config_json, config_hash, git_commit, status (created|running|paused|stopping|stopped), rng_state_json, counters_json` |
| `genomes` | `genome_id PK, parent_ids_json, species_base, generation, birth_index, random_seed, genome_json, content_hash, created_at` |
| `parents` | edges `child_id + parent_id` (composite PK) |
| `births` | `birth_id PK, genome_id, birth_index, generation, mutation_ids_json, created_at, kind: initial|mutation|import` |
| `mutations` | `mutation_id PK, genome_id, kind, path, op, value, scope, provenance_json` |
| `clades` | `clade_id PK, name, founder_genome_id, created_at`; `genome_clades(genome_id, clade_id)` membership. Current rule: the genome in which the first artificial organ appears founds a new clade; descendants inherit the parent's clade unless they found a new one. Root clade: `fba0-root`. |
| `evaluation_jobs` | `job_id PK, genome_id, environment_id, seed, evaluation_tier, backend, duration_ms, requested_traces_json, status (QUEUED|RUNNING|SUCCEEDED|FAILED|CANCELLED|UNKNOWN), priority, created_at, claimed_at, claimed_by_worker, finished_at, attempt, last_error` |
| `evaluations` | `evaluation_id PK, job_id, genome_id, worker_id, backend, seed, batch_size, fitness (placeholder REAL), summary_json, runtime_info_json, config_hash, genome_hash, git_commit, started_at, finished_at, trace_path NULL` |
| `worker_runs` | `worker_run_id PK, worker_id, hostname, gpu_json, runtime_info_json, bench_json, started_at, last_heartbeat_at, status online|offline|lost, completed_jobs, failed_jobs, current_job_id, batch_size` |
| `worker_heartbeats` | `worker_id, at, temperature_c, utilization_pct, vram_used_mb, vram_total_mb, current_job_id` — pruned to last 24h |
| `checkpoints` | `checkpoint_id PK, created_at, reason manual|periodic|stop, manifest_json, path` |
| `experiment_events` | `event_id PK autoincrement, at, type (plain extensible string), severity info|warn|error, payload_json, source` |
| `telemetry_samples` | `id PK, at, source, domain, signal_type, value, normalized_value, delta, confidence, metadata_json` |

## Lineage semantics

Genome lineage records the evolutionary history of the experimental organism.
It remains distinct from Kamimusuhi canonical identity / continuity. A later
MIOA individual may change its neural substrate substantially or distribute
computation across many organs without that implying any automatic change to
Kamimusuhi identity semantics.

The current root name `fba0-root` is historical/scientific provenance: it means
“this lineage was founded from FBA0”, not “FBA0 must remain the dominant organ”.

## Departure measurements

The schema should preserve enough raw evaluation information to distinguish:

1. **functional departure** — the organism becomes less causally dependent on
   FBA0 while remaining viable;
2. **structural replacement** — later experiments actually disable/prune/replace
   ancestral FBA0 structure.

The present schema supports lineage/provenance and raw evaluation summaries but
schema v1 does not itself encode FBA0 structural deletion. Such mutations need
an explicit later schema/semantics version rather than overloading existing
fields.

## Trace storage policy

Full spike trains are **never** stored for ordinary individuals — only
summaries land in `evaluations.summary_json`. Full traces (`.npz` under
`<runs>/<exp>/traces/`, path in `evaluations.trace_path`) are written
only when `requested_traces` is non-empty (elite / novel / debug /
explicit requests per `traces.store_full_for`).
