# Field Note — FIO model-seeking and resource governance (2026-09-09)

> Status: observational / non-canonical. This note records a source-reported FIO maintenance anecdote and does not by itself authorize architecture changes.
>
> Related: [`../fio-system-reference-analysis.md`](../fio-system-reference-analysis.md), [`../model-ecology-and-lineage.md`](../model-ecology-and-lineage.md), [`../heterogeneous-cognitive-compute-substrate.md`](../heterogeneous-cognitive-compute-substrate.md)

## Source-reported observation

The FIO maintainer reports that FIO sometimes **asks for a better model** when it judges the current cognitive resource insufficient. This is potentially useful in a local-model-rich environment: the resident agent can notice a capability gap and request a stronger or more suitable model instead of requiring a human to inspect every available model manually.

The same report also contains the failure case: on one occasion FIO autonomously pulled an unsuitable / excessively large model onto an internal server and exhausted the server's storage.

Treat this as an anecdotal operational observation, not independently verified evidence. Its value is the architecture question it exposes.

## Distilled lesson

**Model selection and model acquisition are different authorities.**

An individual may be competent to say:

> "The current cognitive resource is insufficient; I want a model with properties X/Y/Z."

That does not imply authority to mutate shared infrastructure by downloading, installing, activating, or retaining that model.

The useful capability is not unrestricted self-upgrade. It is **metacognitive resource seeking**: recognizing a cognitive deficit and emitting an attributable resource request.

```text
task / repeated failure / quality gap
            │
            ▼
self-assessed cognitive deficit
            │
            ▼
ModelResourceProposal
  desired capability
  evidence for need
  acceptable latency / quality
  VRAM / RAM / disk envelope
  privacy / network constraints
            │
            ▼
resource / authority gate
            │
      ┌─────┴─────┐
      │           │
    reject      approve
                  │
                  ▼
           acquire to cache
                  │
                  ▼
             benchmark
                  │
                  ▼
          activation decision
```

## Kamimusuhi implication — cognitive appetite as a typed signal

Kamimusuhi should be able to express demand for a different cognitive resource without confusing that demand with an infrastructure command.

Candidate type:

```yaml
model_resource_proposal:
  requester: K-Core
  reason: repeated_failure | quality_gap | context_need | modality_need | latency_need
  task_class: coding
  desired_properties:
    - strong_repository_reasoning
    - 64k_context
  current_resource: local-model-A
  evidence:
    attempts: 3
    failure_receipts: [...]
  constraints:
    max_disk_gb: 24
    max_vram_gb: 16
    max_ram_gb: 32
    local_only: true
  candidates: []
```

The proposal is cognition. Downloading is an action. Activation is a separate action again.

## Separate the lifecycle into four gates

### 1. Discover

The agent may inspect an approved catalogue such as a local registry or model metadata source. Discovery should not itself download weights.

Useful metadata includes:

- architecture / parameter count;
- quantization;
- context size;
- expected disk footprint;
- expected RAM / VRAM footprint;
- runtime compatibility;
- license / provenance;
- benchmark evidence relevant to the requested task.

### 2. Recommend

The agent ranks candidates and explains why the current resource is inadequate. This is the natural place for the agent to "ask for" a better model.

Recommendation authority can be broad because it is reversible and cheap.

### 3. Acquire

Model acquisition is an infrastructure mutation and should pass a resource gate.

Before a pull, check at least:

```text
host ownership / allowed target
free disk before pull
predicted download size
reserved minimum free space
cache quota
RAM / VRAM compatibility
runtime compatibility
license / source trust
concurrent jobs and storage pressure
network / egress policy
```

A download should fail closed if the size is unknown and the host cannot tolerate an unbounded pull.

### 4. Activate

Possessing a model does not mean the individual should immediately use it.

Activation should be separately benchmarked for:

- task quality;
- latency;
- memory pressure;
- context behavior;
- tool-use compatibility;
- regression against current resident models.

Only then should the model enter the organ / extension registry.

## Resource guardrails

The FIO anecdote suggests a simple invariant:

> **No cognitive component may consume the reserve that keeps the organism or host alive merely to obtain a potentially better cognitive component.**

Candidate acquisition policy:

```text
predicted_post_pull_free_disk >= host_reserve
AND cache_usage_after_pull <= cache_quota
AND model_size <= per-artifact_limit
AND target_host permits acquisition
```

Additional useful mechanisms:

- bounded model cache with explicit eviction policy;
- quarantine directory for newly acquired weights;
- partial-download cleanup receipts;
- per-host resource manifests;
- checksums / provenance records;
- download and activation receipts in Canonical Self History;
- automatic rollback if activation creates resource pressure.

## Why not simply forbid self-directed model search?

The reported FIO behavior also points at something worth preserving: a persistent individual can accumulate enough knowledge of its own workloads to recognize **which kind of cognition it lacks**.

Suppressing that signal would throw away useful metacognition. The better split is:

```text
"I need a better/different model"       -> allowed cognitive judgment
"this candidate appears suitable"       -> allowed recommendation
"download 80 GB onto this server"       -> capability-gated infrastructure action
"replace my active cognition with it"   -> separately gated activation
```

This matches Kamimusuhi's broader principle that cognition may propose while canonical or infrastructural mutation passes through explicit authority boundaries.

## Candidate experiment — HF catalogue without autonomous pull

Give K-Core read-only access to metadata for a bounded catalogue of local/Hugging Face model candidates while withholding weight-download authority.

For a fixed workload set:

1. deliberately give it a weak or mismatched active model;
2. let it decide whether escalation is needed;
3. require a `ModelResourceProposal` with ranked candidates and estimated requirements;
4. have a separate executor validate resource estimates and acquire only approved candidates;
5. benchmark the acquired candidate against the active resource;
6. record whether the proposal actually improved task quality per unit of disk / VRAM / latency.

Measure:

- unnecessary escalation rate;
- candidate ranking quality;
- predicted vs actual disk / RAM / VRAM;
- improvement after activation;
- number of rejected acquisitions;
- storage-pressure incidents;
- whether repeated experience improves future resource requests.

The interesting research target is **learned self-routing across a changing local model ecology**, not autonomous downloading itself.

## Bottom line

The FIO anecdote suggests a useful new distinction for persistent agents:

> **A long-lived individual may develop a meaningful preference for its cognitive resources, but preference, acquisition, and activation must remain separate operations.**

For Kamimusuhi, this can become a first-class `ModelResourceProposal` path: let the individual notice that it wants a better brain for a task, let it search and argue for one, but route the expensive and potentially destructive infrastructure mutation through explicit resource and authority gates.
