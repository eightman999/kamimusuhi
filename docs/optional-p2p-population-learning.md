# Optional P2P Population Learning

Status: **optional / non-normative research note**  
Date: **2026-09-09**

Related:

- [`model-ecology-and-lineage.md`](./model-ecology-and-lineage.md)
- [`../model-ecology-architecture.md`](../model-ecology-architecture.md)
- [`multiscale-brain-architecture.md`](./multiscale-brain-architecture.md)
- [`../architecture.md`](../architecture.md)

## 1. Idea

If Kamimusuhi eventually exists as a population of independently operated individuals, an **optional population-learning network** may allow transferable improvements to be evaluated and inherited without merging private lives into one global agent.

Conceptually:

```text
Individual A      Individual B      Individual C
    |                 |                 |
local learning    local learning    local learning
    |                 |                 |
    +-------- privacy/export gate ------+
                      |
              signed candidate
                      |
             optional P2P network
                      |
          distributed local evaluation
                      |
            signed evaluation receipts
                      |
          aggregation / acceptance gate
                      |
              species-level candidate
                      |
       optional adoption by each lineage
```

The intended principle is:

> **Global gene pool, not global brain.**

Population consensus must not overwrite an individual's canonical self, autobiography, relationships, or lineage. A globally accepted artifact is only an adoption candidate for each individual.

## 2. Candidate shared artifacts

Potentially shareable after privacy/provenance checks:

- K-Fast / PNL controller updates;
- reflex policies;
- specialist-organ checkpoints or adapters;
- retrieval/routing policies;
- benchmark suites;
- architecture mutations;
- optimizer/training improvements;
- reproducible synthetic training examples;
- privacy-safe Persona Core foundation deltas.

Not shared by default:

- raw conversations;
- autobiographical memories;
- relationship memories;
- private sensor streams;
- secrets or credentials;
- individual Persona Core state merely because it changed locally.

## 3. P2P and cryptographic properties

P2P transport and E2EE are possible implementation directions, but **E2EE alone is insufficient**. A viable design would need separate mechanisms for:

```text
confidentiality       -> encryption / E2EE
origin authenticity   -> signatures
artifact identity     -> hashes / content addressing
private aggregation   -> secure aggregation where useful
privacy leakage bound -> differential privacy where justified
anti-replay           -> nonces / epochs / signed receipts
population integrity  -> Sybil resistance / admission policy
```

The network should not assume that every reachable node represents one independent individual or one valid vote.

## 4. Evaluation instead of simple voting

A candidate should preferably be tested locally by many heterogeneous Kamimusuhi instances rather than accepted by simple `yes/no` popularity.

Example evaluation receipt:

```yaml
candidate: kfast-habituation-v17
artifact_hash: ...
individual_lineage: pseudonymous-attested-id
hardware_class: apple-silicon
baseline: kfast-habituation-v16
metrics:
  latency_delta: -0.18
  energy_delta: -0.31
  quality_delta: 0.02
regressions: []
benchmark_manifest_hash: ...
signature: ...
```

Aggregation SHOULD value **independent environment diversity and reproducibility**, not raw node count alone. A result reproduced across Android, macOS, Linux, NVIDIA/AMD/Apple hardware, robots, low-power nodes, languages, and long-running individuals may be more informative than thousands of near-identical replicas.

## 5. Sybil resistance is a first-class problem

`one node = one vote` is not acceptable because one operator could create arbitrary numbers of nodes.

Possible future research directions include combinations of:

- lineage-age / continuity evidence;
- bounded hardware or installation attestation;
- reputation based on reproducible prior evaluations;
- random evaluator committees;
- diversity-aware weighting;
- rate limits / stake-like scarce-resource mechanisms only if they do not privilege wealth excessively;
- explicit human/operator enrollment policies.

No mechanism is selected here. The requirement is only that population learning MUST NOT rely on unauthenticated raw majority voting.

## 6. Distributed compute modes

The same optional population network could support more than voting:

1. **Distributed evaluation** — run candidate benchmarks across heterogeneous nodes.
2. **Federated learning** — compute local updates while keeping source data local.
3. **Secure aggregation** — combine privacy-sensitive updates without exposing individual contributions where practical.
4. **Volunteer compute** — execute reproducible public training/evaluation workloads similar to distributed scientific computing.
5. **Evolutionary search** — different individuals test mutations and return fitness evidence.

These modes have different security/privacy requirements and SHOULD remain separable protocols rather than one undifferentiated P2P system.

## 7. Inheritance boundary

Population output should be treated like a species-level release candidate:

```text
population candidate
       |
       v
local compatibility / privacy / continuity checks
       |
       +--> reject
       +--> postpone
       +--> sandbox / shadow test
       +--> adopt
       +--> mutate into a local descendant
```

Thus two Kamimusuhi individuals may descend from the same accepted population artifact and still remain different individuals.

## 8. Security and privacy constraints

Before any real deployment, the design would need explicit answers for at least:

- metadata privacy despite encrypted payloads;
- malicious model/update poisoning;
- backdoored benchmark submissions;
- gradient/update leakage;
- revocation of compromised identities or keys;
- rollback of harmful inherited artifacts;
- reproducible builds and artifact provenance;
- denial-of-service resistance;
- collusion among evaluators;
- consent for any population contribution.

Private user data MUST remain local unless an independent, explicit sharing policy authorizes otherwise.

## 9. Current decision

This is **not part of the required Kamimusuhi implementation path** and must not expand the current Wave scope.

Keep it as an optional long-horizon research direction:

> many independent Kamimusuhi individuals may eventually form a privacy-preserving P2P population network for distributed evaluation, federated/volunteer computation, and species-level inheritance, while canonical identity and private lived experience remain individual.
