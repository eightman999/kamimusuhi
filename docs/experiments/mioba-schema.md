# MIOA experimental schema (legacy `mioba` implementation)

This document describes the schema implemented under `experiments/mioba/`.
The implementation name **MIOBA** is retained for reproducibility, while the
organism-level architecture is **MIOA — Machine Information Organism
Architecture**.

The current genome schema is **v4**. M2 made the ancestral neural substrate an
explicit, registry-resolved part of the genome and added typed organ IR;
M3 added `reflex0` as a deliberately small non-FBA substrate to prove that the
abstraction works end to end. FBA0 remains the M-series founder condition and
the compatibility default for historical records; it is not the definition of
the organism and is not required to remain its permanent central substrate.

This schema still does not claim to represent the complete future MIOA model.
Rich sensory/effectors, habitat interfaces, multi-substrate evolution and
physical substrate replacement remain later experimental work.

## Terminology and compatibility

- **MIOA**: whole organism architecture.
- **MIO**: one organism / individual.
- **FBA0**: ancestral FlyWire-derived substrate used to found the M-series.
- **reflex0**: minimal 48-neuron non-FBA sensor→integrator→motor substrate used
  by M3 to exercise the generic substrate boundary.
- **MIOBA**: historical package / CLI / environment-variable / experiment name.
- Existing paths, DB fields, experiment IDs and recorded M0/M1 artifacts are
  not renamed in place.

## Genome JSON (`schema_version = 4`)

| field | type | meaning |
|---|---|---|
| `genome_id` | str | content hash `b2b:<blake2b-32>` for native records; stored historical IDs are preserved verbatim on migration |
| `schema_version` | int | 4; versions 1–3 are historical M0/M1 records without explicit substrate genes |
| `parent_ids` | list[str] | parent genome IDs; genome lineage, not canonical Kamimusuhi identity |
| `species_base` | str | historical founder label, currently `"fba0"` |
| `generation` / `birth_index` | int | evolutionary position |
| `random_seed` | int | per-genome seed used by evaluation jobs |
| `ancestral_base` | str | ancestral reference name (`flywire-v783-shiu-lif`) |
| `substrates` | list[SubstrateGene] | `{substrate_id, kind, enabled, params}`; registered implementations currently include `fba0` and `reflex0` |
| `artificial_organs` | list[ArtificialOrgan] | `{organ_id, kind, size, params, provenance, enabled, ports, internal, state}`; `ports` / `internal` / `state` are the M2 organ IR fields |
| `attachments` | list[Attachment] | `{attachment_id, source, target, weight_scale, provenance, enabled, signal}`; endpoints are typed and `signal` records the port discipline |
| `parameter_mutations` | list[ParameterMutation] | `{mutation_id, path, op(set|scale|add), value, scope}` |
| `development_rules` | list[dict] | deterministic birth-stage rules applied by `develop()` |
| `plasticity_rules` | list[dict] | heritable rules whose runtime effects live in `LifetimeState`, not in the genome |
| `source_schema_version` | int \| null | authored schema version when a v1–v3 record is migrated; null for native v4 |
| `created_at` | str | ISO8601 UTC |

`OrganProvenance` may name `fba0`, `artificial`, `substrate` or `development`
origins and keeps the relevant parent gene / birth mutation / ancestry fields.

### Substrate semantics

Substrates are resolved through `substrate/registry.py` and the
`SubstrateProtocol`; generic development code does not import the FBA0
implementation.

The two no-substrate-looking cases are intentionally different:

- a legacy document with no substrate records (and the compatibility empty-list
  form used by programmatic genomes) means the implicit FBA0 founder;
- a native v4 genome that declares substrate genes but has every one
  `enabled: false` is **substrate-less**. `develop()` raises
  `NoEnabledSubstrate`; it does not resurrect FBA0.

Duplicate enabled `substrate_id` values are rejected because the developed
phenotype keys substrate neuron populations by ID.

M3's `reflex0` exists as an abstraction proof, not a biological claim. Its
ports and lesion masks are supplied by its adapter and it passes the same
registry → develop → evaluate → lesion/departure → mutation → reproduction →
replay pipeline as the founder substrate.

### Endpoint and organ IR

Attachment endpoints may use:

- explicit substrate ports: `substrate:<id>/<port>`;
- legacy FBA0 endpoint spelling: `fba0:<region>`;
- habitat endpoints: `env:`, `sensor:` and `effector:`;
- organ IDs / organ ports as defined by the organ IR.

`Attachment.signal` records the signal discipline; historical M1 edges are
`event`. Organ ports are typed, allowing later substrates or organs to expose
other disciplines such as the `continuous` drive input used by `reflex0`.

The genome is the recipe, not the runtime body. Birth-stage development rules
are deterministic and heritable. Lifetime plasticity effects live in
per-individual `LifetimeState`, are not hashed into the genome, and are not
written back into the parent's genome when children are produced.

### Migration rule

Loading a v1–v3 document migrates it to the v4 in-memory representation by
injecting the implicit ancestral FBA0 substrate gene and setting
`source_schema_version` to the authored version. The stored `genome_id` is
**never recomputed** during migration. Historical lineage identity therefore
remains the identity recorded when that genome was born.

Children created from migrated parents are native v4 records.

### Content hash rule

`genome_hash(genome)` is `"b2b:" + blake2b(canonical_json(genome without
genome_id, created_at), digest_size=32)`. Canonical JSON uses sorted keys,
compact separators and `ensure_ascii`; `hashlib.blake2b` is used for stable
cross-process behaviour. The `b2b:` prefix is an algorithm/version tag.

The MIOA terminology change does not alter this rule and must not change
existing genome IDs.

## Development and substrate operators

M2 stages substrate-level operators
`DISABLE/BYPASS/PRUNE/REPLACE_SUBSTRATE_REGION`. They are callable through the
operator application path but remain absent from every normal mutation
selection pool. M3 exercises their contracts but does not turn substrate
birth/death or physical replacement on as an evolutionary mechanism.

A genome-disabled substrate port is treated as dangling by development and
structure analysis. Malformed or unknown substrate references fail loudly
rather than silently falling back to FBA0.

## Functional departure records

`m2/departure.py` measures causal dependence under matched seeds using the
intact, sham, founder/reference, organ-ablation and graded substrate-lesion
conditions. Lesion randomness uses its own seed stream. Raw and normalized
metrics are stored under the evaluation summary; the scientific configuration
hash includes functional-departure configuration while execution batching
remains runtime scheduling.

The schema distinguishes:

1. **functional departure** — successful behaviour becomes less dependent on
   the ancestral substrate while remaining viable;
2. **structural replacement** — a later experiment actually removes, bypasses
   or replaces ancestral structure.

M2/M3 establish the machinery and the second-substrate proof. They do not by
themselves establish long-run evolutionary departure from FBA0.

## Future organ and habitat generalization

M2's typed organ IR is deliberately more general than the original neural-only
representation, but the stored field remains `artificial_organs` for
compatibility. Later revisions may make sensory/transducer, processing,
memory, machine-interoceptive, action/effector and communication roles more
explicit.

The habitat is not hidden host state. A display-space sandbox, camera,
microphone, filesystem, network stream or machine-native telemetry source only
becomes part of an experiment when connected through an explicit, recorded
interface. Operational worker telemetry is not automatically an organism
sense.

## SQLite lineage store

`lineage.sqlite` remains the durable experimental lineage/evaluation store.
Core tables include:

| table | purpose |
|---|---|
| `experiments` | experiment configuration, hashes, status, RNG/counters |
| `genomes` / `parents` / `births` / `mutations` | genome records and ancestry |
| `clades` / `genome_clades` | experimental clade membership; historical root remains `fba0-root` |
| `evaluation_jobs` | queued/running/terminal evaluation work and claim metadata |
| `evaluations` | results, summaries, runtime identity, hashes and optional trace path |
| `worker_runs` / `worker_heartbeats` | execution-lane and heartbeat records |
| `checkpoints` | resumable experiment checkpoints |
| `experiment_events` | extensible experiment event log |
| `telemetry_samples` | operational MIE telemetry samples |

The DB's migration/versioning machinery is separate from the genome JSON
`schema_version`; do not infer one version number from the other.

## Lineage and identity boundary

Genome lineage records evolutionary ancestry of the experimental organism. It
is distinct from Kamimusuhi canonical identity / continuity. The historical
`fba0-root` clade name means “founded from FBA0”; it does not assert that FBA0
must remain the dominant organ or substrate.

## Trace storage policy

Full spike trains are not stored for ordinary individuals. Evaluation summaries
live in the DB; full `.npz` traces under the run directory are written only
when explicitly requested by the experiment's trace policy.
