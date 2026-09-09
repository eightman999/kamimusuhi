# Field Note — FIO model-seeking and resource governance (2026-09-09)

> Status: observational / non-canonical. This note records source-reported FIO maintenance anecdotes and does not by itself authorize architecture changes.
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

---

## 2026-09-10 update — from monolithic chimera to explicit multi-model composition

A later source-reported conversation adds an important piece of FIO's design history.

The maintainer explicitly clarified that the current system should **not be understood as one foundation model with a character prompt attached**. Several models are connected on the provider / routing side. The exact present composition was uncertain in the conversation itself, so this note intentionally does not preserve an unverified model list or exact topology.

The same maintainer described an earlier desktop/Ollama period in which multiple local models were combined more aggressively as a **chimera-style model experiment**. That experiment reportedly produced communication-like output whose meaning was interpretable to the maintainer but opaque to other people. The experiment was discontinued.

This should not be overinterpreted. In particular, this anecdote is **not sufficient evidence of a genuinely emergent private language**. Similar symptoms could arise from tokenizer incompatibility, special-token leakage, prompt conventions, corrupted composition, decoding behavior, representation mismatch, or other implementation artifacts. The source also mentioned human-factors concerns that contributed to stopping the experiment; this note deliberately records no diagnosis and makes no causal claim between model behavior and any person's health.

### Distillation: do not fuse the individual into an opaque model mixture

The strongest transferable lesson is architectural:

> **Borrow cognition from many models if useful, but keep identity, authority, provenance, and resource ownership outside the mixture.**

There is an important difference between two designs:

```text
Monolithic / opaque chimera

model A ─┐
model B ─┼── opaque combination ──> one apparent mind
model C ─┘
                    │
                    └── difficult to attribute, inspect, replace, or roll back
```

and:

```text
Explicit multi-model cognitive graph

                ┌── model A / role A
Individual ─────┼── model B / role B
identity        └── model C / role C
   │                    │
   │              attributed outputs
   │                    │
   └──── policy / router / provenance / memory gate
```

The second form fits Kamimusuhi much better.

A model may be replaced without replacing the individual. A model may fail without corrupting lineage. A model may disagree with another model without either output automatically becoming canonical self-state. The Continuity Kernel, canonical history, policy gates, and model/resource registry remain authoritative outside the replaceable cognitive substrate.

### Design principle — `individual != model != ensemble`

Kamimusuhi already separates persistent identity from external cognitive resources. The FIO anecdote strengthens the need to preserve that boundary even when several models are composed.

Candidate invariant:

```text
IndividualIdentity != ActiveModel
IndividualIdentity != ModelEnsemble
ModelOutput       != SelfBelief
ModelConsensus    != CanonicalMutation
```

Consensus among several models can increase confidence, but it still does not grant identity authority.

### Inter-model communication must be attributable

If multiple models collaborate, every hop should retain enough provenance to answer:

```text
who produced this?
using which model / quant / runtime?
under which role and prompt contract?
from which input/evidence?
was it transformed by another model?
what confidence / uncertainty was attached?
which policy admitted it into the next layer?
```

A useful envelope could look like:

```yaml
cognitive_artifact:
  producer_org: planning
  model_ref: qwen-local-x
  runtime_ref: llama.cpp-build-y
  input_refs: [...]
  artifact_type: hypothesis
  visibility: internal
  confidence: 0.61
  transformation_chain:
    - producer_org: critic
      model_ref: model-z
      operation: critique
  authority: non_canonical
```

The content can remain flexible; the envelope should not.

### Opaque internal representation: allow experimentally, distrust operationally

An artificial cognitive system may discover compressed or idiosyncratic representations that are efficient for machine-to-machine communication. Kamimusuhi should not require every intermediate representation to be pleasant natural language.

But **opacity cannot silently increase authority**.

Candidate rule:

```text
opaque internal artifact
        │
        ├── may remain transient internal computation
        ├── may be transformed into a typed / inspectable artifact
        └── MUST NOT directly become canonical memory, self-state, or external action
```

If an internal protocol becomes intentionally non-human-readable, prefer one of:

1. a specified schema / codec with reversible inspection tooling;
2. an explicit typed latent/artifact interface whose producer and consumer are known;
3. quarantine as an experimental representation with no canonical write authority.

Do not mistake "the models seem to understand each other" for a sufficient interface contract.

### Resource governance and composition governance are the same family of problem

The earlier model-pull incident and the chimera experiment point to the same higher-level rule:

```text
cognitive desire / exploration
            │
            ▼
proposal
            │
            ▼
resource + composition policy
            │
      ┌─────┴────────┐
      │              │
   reject          sandbox
                       │
                       ▼
                  benchmark
                       │
                       ▼
                 attributed activation
```

The individual may request:

- a stronger model;
- a specialist model;
- a new model-to-model connection;
- a different routing policy;
- an experimental composition.

Those are legitimate metacognitive proposals. Creating the connection, pulling weights, altering resource allocation, or changing the production cognitive graph remains gated infrastructure mutation.

### Candidate experiment — explicit composition vs chimera

For a fixed task set, compare:

1. one strong model;
2. explicit multi-model routing with typed intermediate artifacts;
3. an experimental fused/mixed composition where technically possible.

Measure:

- task quality;
- reproducibility;
- attribution completeness;
- failure localization time;
- rollback success;
- resource cost;
- rate of malformed / opaque intermediate artifacts;
- contamination of memory or self-state;
- behavior after replacing one component model.

The research question is not merely whether an ensemble scores higher. It is whether **cognitive plurality can improve capability while continuity remains independent of the constituent models**.

### Candidate experiment — opacity tripwire

Introduce a monitor at model-to-model boundaries that does not attempt to understand all content, but detects loss of interface discipline:

- unexpected encoding / token-like garbage;
- schema escape;
- sudden language/distribution shift;
- untraceable producer;
- missing transformation history;
- artifact type mismatch;
- opaque content attempting a canonical write or external action.

The tripwire should not automatically ban novel representations. It should downgrade authority, preserve evidence, and route the artifact to inspection/sandboxing.

## Bottom line

The combined FIO anecdotes suggest two complementary rules for persistent agents:

> **A long-lived individual may develop meaningful preferences for its cognitive resources, but preference, acquisition, composition, and activation must remain separate operations.**

and:

> **Use multiple models as replaceable, attributed cognitive organs rather than allowing an opaque model mixture to become the definition of the individual.**

For Kamimusuhi, this supports a first-class `ModelResourceProposal` / composition-governance path: let the individual notice that it wants a better brain or a different cognitive topology, let it search and argue for one, but route expensive, opaque, or identity-affecting changes through explicit resource, provenance, sandbox, and authority gates.