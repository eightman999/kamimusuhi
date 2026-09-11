-- MIOBA experiment lineage database (sqlite, WAL). Single writer = coordinator.
CREATE TABLE IF NOT EXISTS schema_version (
    version INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS experiments (
    experiment_id   TEXT PRIMARY KEY,
    created_at      TEXT NOT NULL,
    config_json     TEXT NOT NULL,
    config_hash     TEXT NOT NULL,
    -- hash of the result-affecting subset of the config (backend, env,
    -- population, mutation, fba, evaluation, fitness) vs the operational rest
    scientific_config_hash TEXT,
    runtime_config_hash    TEXT,
    git_commit      TEXT,
    status          TEXT NOT NULL DEFAULT 'created',
    rng_state_json  TEXT,
    counters_json   TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS genomes (
    experiment_id   TEXT NOT NULL,
    genome_id       TEXT PRIMARY KEY,
    parent_ids_json TEXT NOT NULL DEFAULT '[]',
    species_base    TEXT NOT NULL,
    generation      INTEGER NOT NULL,
    birth_index     INTEGER NOT NULL,
    random_seed     INTEGER NOT NULL,
    genome_json     TEXT NOT NULL,
    content_hash    TEXT NOT NULL,
    created_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS genomes_exp ON genomes(experiment_id, generation);

CREATE TABLE IF NOT EXISTS parents (
    experiment_id TEXT NOT NULL,
    child_id      TEXT NOT NULL,
    parent_id     TEXT NOT NULL,
    PRIMARY KEY (child_id, parent_id)
);

CREATE TABLE IF NOT EXISTS births (
    experiment_id    TEXT NOT NULL,
    birth_id         TEXT PRIMARY KEY,
    genome_id        TEXT NOT NULL,
    birth_index      INTEGER NOT NULL,
    generation       INTEGER NOT NULL,
    mutation_ids_json TEXT NOT NULL DEFAULT '[]',
    created_at       TEXT NOT NULL,
    kind             TEXT NOT NULL CHECK (kind IN ('initial','mutation','import'))
);
CREATE INDEX IF NOT EXISTS births_exp ON births(experiment_id);

CREATE TABLE IF NOT EXISTS mutations (
    experiment_id   TEXT NOT NULL,
    mutation_id     TEXT PRIMARY KEY,
    genome_id       TEXT NOT NULL,
    kind            TEXT NOT NULL,
    path            TEXT,
    op              TEXT,
    value           REAL,
    scope           TEXT,
    provenance_json TEXT NOT NULL DEFAULT '{}'
);

CREATE TABLE IF NOT EXISTS clades (
    experiment_id     TEXT NOT NULL,
    clade_id          TEXT PRIMARY KEY,
    name              TEXT NOT NULL,
    founder_genome_id TEXT,
    created_at        TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS genome_clades (
    experiment_id TEXT NOT NULL,
    genome_id     TEXT NOT NULL,
    clade_id      TEXT NOT NULL,
    PRIMARY KEY (genome_id, clade_id)
);

CREATE TABLE IF NOT EXISTS evaluation_jobs (
    experiment_id         TEXT NOT NULL,
    job_id                TEXT PRIMARY KEY,
    genome_id             TEXT NOT NULL,
    environment_id        TEXT NOT NULL,
    seed                  INTEGER NOT NULL,
    evaluation_tier       TEXT NOT NULL,
    backend               TEXT NOT NULL,
    duration_ms           REAL NOT NULL,
    requested_traces_json TEXT NOT NULL DEFAULT '[]',
    status                TEXT NOT NULL CHECK (status IN
                          ('QUEUED','RUNNING','SUCCEEDED','FAILED','CANCELLED','UNKNOWN')),
    priority              INTEGER NOT NULL DEFAULT 0,
    created_at            TEXT NOT NULL,
    claimed_at            TEXT,
    claimed_by_worker     TEXT,
    finished_at           TEXT,
    attempt               INTEGER NOT NULL DEFAULT 0,
    last_error            TEXT,
    -- scientific replicate count (evaluation.replicates), fixed per experiment
    replicates            INTEGER NOT NULL DEFAULT 1,
    -- deterministic id of the worker result that finished this job;
    -- a re-delivery with the same id is acknowledged, not re-applied
    result_id             TEXT
);
CREATE INDEX IF NOT EXISTS jobs_status ON evaluation_jobs(experiment_id, status, priority);

CREATE TABLE IF NOT EXISTS evaluations (
    experiment_id      TEXT NOT NULL,
    evaluation_id      TEXT PRIMARY KEY,
    job_id             TEXT NOT NULL,
    genome_id          TEXT NOT NULL,
    worker_id          TEXT,
    backend            TEXT,
    seed               INTEGER,
    batch_size         INTEGER,
    fitness            REAL,
    summary_json       TEXT NOT NULL DEFAULT '{}',
    runtime_info_json  TEXT NOT NULL DEFAULT '{}',
    config_hash        TEXT,
    scientific_config_hash TEXT,
    genome_hash        TEXT,
    git_commit         TEXT,
    started_at         TEXT,
    finished_at        TEXT,
    trace_path         TEXT,
    environment_id     TEXT,
    duration_ms        REAL,
    -- logical dataset identity {dataset_id, version, manifest_hash, region_mode}
    dataset_json       TEXT NOT NULL DEFAULT '{}',
    device             TEXT,
    requested_replicates INTEGER,
    completed_replicates INTEGER,
    -- GPU lanes per chunk (operational; must not affect fitness)
    execution_batch_size INTEGER,
    replicate_seeds_json TEXT NOT NULL DEFAULT '[]',
    result_id          TEXT
);
CREATE INDEX IF NOT EXISTS evals_genome ON evaluations(experiment_id, genome_id);

CREATE TABLE IF NOT EXISTS worker_runs (
    experiment_id      TEXT NOT NULL,
    worker_run_id      TEXT PRIMARY KEY,
    worker_id          TEXT NOT NULL,
    hostname           TEXT,
    gpu_json           TEXT NOT NULL DEFAULT '[]',
    runtime_info_json  TEXT NOT NULL DEFAULT '{}',
    bench_json         TEXT NOT NULL DEFAULT '[]',
    started_at         TEXT NOT NULL,
    last_heartbeat_at  TEXT,
    status             TEXT NOT NULL CHECK (status IN ('online','offline','lost')),
    completed_jobs     INTEGER NOT NULL DEFAULT 0,
    failed_jobs        INTEGER NOT NULL DEFAULT 0,
    current_job_id     TEXT,
    batch_size         INTEGER,
    device             TEXT
);
CREATE INDEX IF NOT EXISTS worker_runs_exp ON worker_runs(experiment_id, worker_id);

CREATE TABLE IF NOT EXISTS worker_heartbeats (
    experiment_id   TEXT NOT NULL,
    worker_id       TEXT NOT NULL,
    at              TEXT NOT NULL,
    temperature_c   REAL,
    utilization_pct REAL,
    vram_used_mb    REAL,
    vram_total_mb   REAL,
    current_job_id  TEXT
);
CREATE INDEX IF NOT EXISTS hb_worker ON worker_heartbeats(experiment_id, worker_id, at);

CREATE TABLE IF NOT EXISTS checkpoints (
    experiment_id TEXT NOT NULL,
    checkpoint_id TEXT PRIMARY KEY,
    created_at    TEXT NOT NULL,
    reason        TEXT NOT NULL CHECK (reason IN ('manual','periodic','stop')),
    manifest_json TEXT NOT NULL,
    path          TEXT
);

CREATE TABLE IF NOT EXISTS experiment_events (
    experiment_id TEXT NOT NULL,
    event_id      INTEGER PRIMARY KEY AUTOINCREMENT,
    at            TEXT NOT NULL,
    type          TEXT NOT NULL,
    severity      TEXT NOT NULL CHECK (severity IN ('info','warn','error')),
    payload_json  TEXT NOT NULL DEFAULT '{}',
    source        TEXT
);
CREATE INDEX IF NOT EXISTS events_exp ON experiment_events(experiment_id, event_id);

CREATE TABLE IF NOT EXISTS telemetry_samples (
    experiment_id    TEXT NOT NULL,
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    at               TEXT NOT NULL,
    source           TEXT NOT NULL,
    domain           TEXT NOT NULL,
    signal_type      TEXT NOT NULL,
    value            REAL,
    normalized_value REAL,
    delta            REAL,
    confidence       REAL,
    metadata_json    TEXT NOT NULL DEFAULT '{}'
);
CREATE INDEX IF NOT EXISTS telemetry_exp ON telemetry_samples(experiment_id, id);
