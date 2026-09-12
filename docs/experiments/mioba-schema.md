# MIOBA schema: genome JSON + lineage SQLite

## Genome JSON (`schema_version = 4`)

| field | type | meaning |
|---|---|---|
| `genome_id` | str | content hash `b2b:<blake2b-32>`, filled by `finalize()`; a stored field, never recomputed |
| `schema_version` | int | 4 (1-3: M0/M1 records without `substrates`) |
| `parent_ids` | list[str] | parent genome_ids (genome_lineage, not canonical identity) |
| `species_base` | str | `"fba0"` |
| `generation` / `birth_index` | int | evolutionary position |
| `random_seed` | int | per-genome seed used by evaluation jobs |
| `ancestral_base` | str | FBA0 reference name (`flywire-v783-shiu-lif`) |
| `substrates` | list[SubstrateGene] | v4+: `{substrate_id, kind, enabled, params}` — which substrate implementations the organism is built on. v1-v3 documents have no such field; loading one assigns the implicit ancestral FBA0 gene |
| `artificial_organs` | list[ArtificialOrgan] | `{organ_id, kind, size, params, provenance, enabled, ports, internal, state}`; `ports`/`internal`/`state` are the M2 organ IR fields |
| `attachments` | list[Attachment] | `{attachment_id, source, target, weight_scale, provenance, enabled, signal}`; endpoints are typed (`substrate:<id>/<port>`, legacy `fba0:<region>`, `env:`/`sensor:`/`effector:` names, or a bare organ_id); `signal` is the port discipline (`event` for every M1 edge) |
| `parameter_mutations` | list[ParameterMutation] | `{mutation_id, path, op(set|scale|add), value, scope}`; scope `global`/`region:<x>`/`organ:<id>` |
| `development_rules` | list[dict] | birth-stage ops applied deterministically at `develop()` (ADD/GROW/SCALE_ORGAN_AT_BIRTH); lifetime-stage ops are deferred to the plasticity runtime |
| `plasticity_rules` | list[dict] | heritable rules whose *effects* live in per-individual `LifetimeState`, never in the genome |
| `source_schema_version` | int \| null | the version a document was authored in, set when loading migrates a v1-v3 record; null for natively-authored records |
| `created_at` | str | ISO8601 UTC |

`OrganProvenance`: `{origin: "fba0"|"artificial"|"substrate"|
"development", parent_gene, birth_mutation_id, ancestry}`.

### Migration rule

`from_dict` on a v1-v3 document (no `substrates` field) injects
`SubstrateGene("fba0", "flywire-v783-shiu-lif")`, sets
`source_schema_version` to the authored version and
`schema_version` to 4. The record keeps its stored `genome_id`
verbatim — historical identities are never recomputed, so a migrated
lineage stays hash-identical to the day it was born.

### Content hash rule

`genome_hash(genome)` = `"b2b:" + blake2b(canonical_json(genome without
genome_id, created_at), digest_size=32)`. Canonical JSON = sorted keys,
compact separators, `ensure_ascii`. `hashlib.blake2b` (stdlib) is used so
the hash is deterministic across processes; the `b2b:` prefix is a
version tag allowing a later BLAKE3 (`b3:`) migration without ambiguity.

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
| `clades` | `clade_id PK, name, founder_genome_id, created_at`; `genome_clades(genome_id, clade_id)` membership. Rule: the genome in which the first artificial organ appears founds a new clade; descendants inherit the parent's clade unless they found a new one. Root clade: `fba0-root`. |
| `evaluation_jobs` | `job_id PK, genome_id, environment_id, seed, evaluation_tier, backend, duration_ms, requested_traces_json, status (QUEUED|RUNNING|SUCCEEDED|FAILED|CANCELLED|UNKNOWN), priority, created_at, claimed_at, claimed_by_worker, finished_at, attempt, last_error` |
| `evaluations` | `evaluation_id PK, job_id, genome_id, worker_id, backend, seed, batch_size, fitness (placeholder REAL), summary_json, runtime_info_json, config_hash, genome_hash, git_commit, started_at, finished_at, trace_path NULL` |
| `worker_runs` | `worker_run_id PK, worker_id, hostname, gpu_json, runtime_info_json, bench_json, started_at, last_heartbeat_at, status online|offline|lost, completed_jobs, failed_jobs, current_job_id, batch_size` |
| `worker_heartbeats` | `worker_id, at, temperature_c, utilization_pct, vram_used_mb, vram_total_mb, current_job_id` — pruned to last 24h |
| `checkpoints` | `checkpoint_id PK, created_at, reason manual|periodic|stop, manifest_json, path` |
| `experiment_events` | `event_id PK autoincrement, at, type (plain extensible string), severity info|warn|error, payload_json, source` |
| `telemetry_samples` | `id PK, at, source, domain, signal_type, value, normalized_value, delta, confidence, metadata_json` |

## Trace storage policy

Full spike trains are **never** stored for ordinary individuals — only
summaries land in `evaluations.summary_json`. Full traces (`.npz` under
`<runs>/<exp>/traces/`, path in `evaluations.trace_path`) are written
only when `requested_traces` is non-empty (elite / novel / debug /
explicit requests per `traces.store_full_for`).
