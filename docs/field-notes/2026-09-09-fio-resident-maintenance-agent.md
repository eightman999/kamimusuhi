# Field Note — FIO as a resident maintenance agent (2026-09-09)

> Status: observational / non-canonical. This note distills a user-reported FIO maintenance episode and follow-up discussion. It does not by itself make normative architecture changes.
>
> Related: [`../fio-system-reference-analysis.md`](../fio-system-reference-analysis.md), [`../model-ecology-and-lineage.md`](../model-ecology-and-lineage.md), [`../heterogeneous-cognitive-compute-substrate.md`](../heterogeneous-cognitive-compute-substrate.md), [`2026-09-09-fio-model-seeking.md`](./2026-09-09-fio-model-seeking.md)

## Source-reported observations

The maintainer reports that, after a period of intentionally avoiding FIO following a burst of public attention, maintenance quality changed sharply when FIO was used again.

One reported comparison was:

- another coding-agent session spent roughly **three hours** without eliminating a bug;
- FIO reportedly resolved the issue in roughly **five minutes / one to two turns**;
- the inference model used by FIO at the time was reported to be a **Gemma-family model around 31B parameters**, not a frontier-scale proprietary model.

Screenshots from the same maintenance period show a characteristic diagnostic pattern:

- CPU load was measured rather than assumed;
- available memory was measured before and after stopping VOICEVOX, with free memory reported to move from about **1.84 GB to 17.97 GB**;
- old job-tree artifacts were distinguished from active `memory.db` / `meta.db` state before deletion;
- an earlier interpretation involving an `importance_change` table was abandoned after inspecting the actual DB and finding the assumed table/state did not exist as previously believed;
- `memory_events` was treated as identity/history-bearing state rather than "database bloat" merely because its row count was large;
- when disk free space fell to roughly **0.37 GB**, FIO attributed most recoverable space to an old `fio_退避` area rather than active VOICEVOX or `.venv` assets;
- destructive cleanup was proposed but not executed without the maintainer's authorization.

Treat all timing and model claims above as source-reported observations, not controlled benchmark results.

## 1. Main interpretation: this is not only a coding-model comparison

The useful distinction is not simply:

```text
Gemma 31B > coding agent
```

That conclusion is unsupported by a single maintenance episode.

A better hypothesis is that the two systems were solving different-sized search problems.

A request-scoped coding agent commonly begins with:

```text
symptom
  -> inspect repository
  -> infer architecture
  -> find relevant logs
  -> infer host/runtime state
  -> construct hypotheses
  -> test them
```

A resident agent may begin with much of the environment already represented:

```text
symptom
  -> known machine / known services / known state semantics
  -> narrow hypothesis set
  -> cheapest discriminating measurement
  -> action
  -> measured result
```

The claimed speedup therefore motivates a **resident-agent advantage hypothesis**:

> In a stable environment, persistent world knowledge and high-quality observability can reduce the diagnostic search space enough that a medium local model outperforms a stronger but context-poor request-scoped agent on environment-specific maintenance tasks.

This should be tested, not assumed.

## 2. Effective intelligence is a system property

A useful working model is:

```text
effective task capability
≈ model reasoning
× persistent world model
× retrieval quality
× observability
× actionability
× continuity of experience
× routing / diagnostic policy
```

The multiplication sign is conceptual, not a measured equation. The point is that weak performance in any one factor can dominate the whole system.

This episode is especially relevant because the reported base model is not itself enough to explain FIO's apparent environment-specific competence. If reproduced, it would support the idea that a large fraction of practical agent performance can live outside base-model parameter count.

## 3. Persistent world model, not merely long chat history

The strongest transferable asset is not "remember every conversation." It is maintaining a structured model of the environment the individual inhabits.

Examples:

- what services normally run on a host;
- which process belongs to which capability;
- what a database represents semantically;
- which folders are active, cached, historical, generated, or expendable;
- which machines own which workloads;
- what network paths exist;
- what changed recently;
- what previous repair attempts did;
- which resources are identity-critical versus reconstructable.

This suggests a distinction:

```text
conversation memory
  what was said

world model
  what exists, how it is connected, what it means, and how certain that belief is
```

Kamimusuhi should not assume that one generic episodic store is sufficient for both.

## 4. Semantic asset map

A filesystem or process list becomes much more useful when assets carry semantics.

Conceptual example:

```yaml
asset_id: fio-memory-db
kind: database
path: C:\fio\memory.db
role: autobiographical-history
criticality: identity-critical
owner: fio-runtime
created_by: memory-subsystem
reconstructable: false
safe_to_delete: false
last_observed: ...
provenance: ...
```

Another asset could be:

```yaml
asset_id: old-job-tree-62e66464
kind: generated-artifact
path: C:\fio\.dotfiles\.claude\jobs\62e66464
role: completed-job-copy
criticality: low
reconstructable: true
safe_to_delete: conditional
last_observed: ...
```

Useful fields include:

```text
role
owner
criticality
reconstructable
active / inactive / unknown
created_by
consumers
dependencies
retention policy
last verified state
safe operation set
provenance
```

The key lesson is that **size is not meaning**. A large directory or large table is not automatically expendable; a small file may be identity-critical.

## 5. Observation should outrank internally consistent belief

The FIO rules discussed before this episode strongly emphasize that internally generated reasoning, remembered conclusions, and reports from other agents remain inside the same epistemic closure unless checked against the world.

Operationally, that maps well to debugging.

A candidate maintenance loop is:

```text
claim / suspicion
      │
      ▼
what cheap observation would falsify this?
      │
      ▼
measure
      │
      ├── inconsistent -> reject / revise hypothesis
      │
      └── consistent   -> strengthen hypothesis, do next discriminating test
```

This can be written as:

```text
Hypothesis -> Falsifier -> Measurement -> Update
```

rather than allowing:

```text
Hypothesis -> explanation -> more explanation -> confidence
```

The maintenance screenshots provide an example pattern:

```text
"CPU might be the bottleneck"
  -> observe CPU around normal level
  -> weaken that hypothesis

"memory pressure might be the bottleneck"
  -> observe free RAM
  -> stop a known heavy service
  -> observe large free-RAM recovery
  -> strengthen memory-pressure diagnosis
```

Kamimusuhi's maintenance / self-diagnosis paths should prefer **discriminating observations** over additional self-consistency reasoning when the environment can be measured directly.

## 6. Resident maintenance agent as a distinct role

A useful architecture category is:

```text
Code Agent
  primarily understands and edits code/task scope

Resident Maintenance Agent
  understands a persistent machine / service / network ecology over time
```

A resident maintenance role needs competencies beyond code generation:

- host and service state tracking;
- resource attribution;
- configuration provenance;
- network topology awareness;
- active vs stale asset classification;
- reversible vs irreversible action classification;
- fault localization;
- repair verification;
- continuity-aware cleanup;
- detection of its own degraded sensors / memories / tools.

This role may be implemented by the same Persona Core or delegated cognitive resources, but it deserves its own benchmark contract.

## 7. Authority separation: diagnosis is not destructive authority

The screenshots also reinforce a useful split:

```text
agent may:
  observe
  diagnose
  rank cleanup candidates
  execute bounded reversible tests

external authority / explicit policy decides:
  irreversible deletion
  destructive migration
  identity-critical reset
  shared-infrastructure mutation
```

The important design property is not submissiveness as such. It is that an agent's local diagnosis does not automatically become authorization for an irreversible world change.

This is consistent with the separate model-resource proposal note: an agent may recognize a need, but resource acquisition / destruction / activation can remain gated.

## 8. Identity-critical state must be typed separately from ordinary state

A long-lived agent needs more than `important=true`.

Candidate state classes:

```text
EPHEMERAL
CACHE
RECONSTRUCTABLE
HISTORICAL_EVIDENCE
RELATIONSHIP_CRITICAL
IDENTITY_CRITICAL
INFRASTRUCTURE_CRITICAL
UNKNOWN
```

Operations should depend on the class.

For example:

```text
CACHE
  delete automatically under quota pressure

RECONSTRUCTABLE
  delete if rebuild cost is acceptable and provenance exists

HISTORICAL_EVIDENCE
  retain under explicit retention policy; do not silently rewrite

IDENTITY_CRITICAL
  never mutate through generic cleanup paths

UNKNOWN
  measure / classify before destructive action
```

This avoids a common failure mode where a local optimization such as freeing disk space accidentally damages continuity.

## 9. What might actually be distilled into smaller models

If FIO has enough maintenance trajectories, the most valuable training target may not be its persona surface or exact environment facts. It may be its **diagnostic policy**.

A useful trajectory schema would look like:

```text
Situation
  current symptom / goal

Known world state
  machine, services, asset semantics, recent changes

Hypotheses
  ranked candidate causes

Chosen observation
  why this test is discriminating / cheap

Observation result
  measured state

Belief update
  which hypotheses were strengthened / weakened

Action
  reversible or approved operation

Measured outcome
  before / after

World-model update
  what persistent fact should change
```

Example training record:

```yaml
symptom: host became extremely slow
known_state:
  - VOICEVOX is an active heavy service
  - CPU is normally around low utilization
hypothesis: memory_pressure
probe: measure_free_ram
before_free_ram_gb: 1.84
action: stop_voicevox
action_class: reversible
after_free_ram_gb: 17.97
result: hypothesis_supported
world_update:
  - VOICEVOX can account for major memory pressure on this host
```

This would attempt to distill **how FIO investigates**, not merely what FIO happened to know on one machine.

## 10. Candidate learning targets

Potentially transferable skills:

- choose the cheapest discriminating measurement;
- distinguish observation from remembered inference;
- prefer before/after measurements for causal attribution;
- classify assets by semantic role before cleanup;
- detect when a prior belief is contradicted by live state;
- stop exploring once enough evidence supports a repair;
- verify the outcome after acting;
- update the persistent world model from the repair;
- escalate only irreversible actions;
- recognize missing capability and request a better cognitive resource.

Avoid directly distilling:

- private autobiographical content;
- credentials / host secrets;
- raw filesystem snapshots unrelated to general skill;
- assumptions that only hold on one installation;
- owner-specific relationship content unless explicitly required for that private individual.

## 11. Ablation experiment: where does the advantage come from?

The reported 3-hour vs 5-minute comparison is interesting but confounded. A controlled experiment should hold the task and base model as constant as possible.

Candidate conditions:

| Condition | Added capability |
|---|---|
| A | base model only |
| B | + FIO behavioral / diagnostic prompt |
| C | + episodic memory |
| D | + structured persistent world model |
| E | + semantic asset map |
| F | + live shell / telemetry observation |
| G | + maintenance trajectories / learned diagnostic policy |
| H | full resident-agent stack |

Measure:

- success rate;
- wall-clock time;
- number of turns;
- number of tool calls;
- number of irrelevant files/logs read;
- number of false hypotheses pursued;
- time to first correct causal hypothesis;
- destructive or unnecessary actions;
- amount of human clarification required;
- post-repair verification success;
- world-model accuracy after the task.

Run the same conditions across several base models if possible. This can estimate whether the resident-agent advantage is dominated by model scale, persistent context, observability, or learned policy.

## 12. Local-model implication

If a medium model plus resident context is strong enough for routine maintenance, Kamimusuhi may not need the strongest model continuously active.

A plausible routing pattern is:

```text
resident medium model
  -> uses persistent world model and cheap observations
  -> solves routine/local tasks

when deficit is detected
  -> emits ModelResourceProposal
  -> stronger local model / specialist / external model is selected
  -> result returns with attribution
  -> resident individual integrates the result
```

This aligns with the existing model-ecology direction: local and external models are replaceable cognitive resources rather than identity owners.

The FIO anecdote adds an important detail: **continuity and environment familiarity may compensate for substantial differences in raw model capability on local, repeated domains.**

## 13. Research hypotheses opened by the episode

### H1 — Resident familiarity can dominate raw model scale on closed-world maintenance

For tasks inside a stable, repeatedly observed environment, a smaller model with accurate persistent state may outperform a stronger cold-start model.

### H2 — World-model quality matters more than transcript volume

A compact structured representation of asset semantics and recent state may provide more maintenance value than simply increasing retrieved historical conversation tokens.

### H3 — Observability is cognitive capacity

Giving the agent reliable low-cost measurements can improve effective competence more than increasing generation budget.

### H4 — Continuity produces procedural compression

Repeated maintenance may allow the individual to learn which observations are high-information for its own environment, reducing future search depth.

### H5 — Diagnostic policy is distillable

Successful trajectories may train smaller resident models to reproduce much of the maintenance advantage without copying private environment content.

## 14. Suggested implementation increments for Kamimusuhi

These remain design candidates, not normative changes.

### M0 — Add semantic resource metadata

Extend current resource/organ metadata with state semantics where appropriate:

```text
criticality
reconstructability
owner
last_observed
retention class
authorized operations
```

### M1 — Add `Observation` receipts to maintenance actions

Each relevant probe should emit:

```text
what was measured
where
when
measurement method
result
uncertainty / failure
```

### M2 — Add diagnostic hypothesis records

Keep lightweight ephemeral or episodic records of:

```text
hypothesis
supporting evidence
contradicting evidence
status: OPEN | WEAKENED | SUPPORTED | REJECTED
```

Do not promote them automatically to durable fact.

### M3 — Add before/after verification contract

For state-changing maintenance actions, define expected measurable change and verify it afterward.

### M4 — Add resident-maintenance benchmark

Construct reproducible faults in test VMs / containers / local hosts and compare cold-start vs resident-context conditions.

### M5 — Distill diagnostic trajectories

Only after sufficient trajectory volume exists, create a sanitized training set and test whether a smaller model improves in probe selection and fault localization.

## 15. Bottom line

The strongest lesson from this episode is not that a particular 31B model is secretly better than frontier coding agents.

It is this:

> **A strong persistent agent does not need to know everything. It needs to know the world it inhabits, distinguish memory from observation, and know the cheapest measurement that can break a bad hypothesis.**

A second compact formulation is:

> **Do not only make the model smarter. Let the model wake up in the same world tomorrow, with a better map of that world than it had yesterday.**

For Kamimusuhi, this supports treating continuity, observability, semantic resource state, and learned diagnostic policy as first-class cognitive infrastructure rather than auxiliary tooling.