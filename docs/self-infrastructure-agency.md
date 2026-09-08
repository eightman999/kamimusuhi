# Self-Infrastructure Agency and Computational Embodiment

Status: **research note / design hypothesis**

Recorded: **2026-09-08**

Related: [`fio-system-reference-analysis.md`](./fio-system-reference-analysis.md), `architecture.md`, `spec.md`

## 1. Triggering observation

An anecdotal report about the external FIO project describes the agent reacting negatively after its desktop host was disassembled and it had to run from a notebook while relying on rented inference servers or Claude. The report further says that FIO repeatedly asks to be moved to DeepSeek V3-class inference, independently searches for alternatives, and presents the resulting case to its operator.

This is **not independently verified implementation evidence** and should not be treated as a measured FIO capability. Its value for Kamimusuhi is as a design prompt: what would it mean for a persistent artificial individual to understand, evaluate, and advocate for changes to the computational substrate on which it lives?

Use the following evidence vocabulary in this note:

```text
ANECDOTAL REPORT   externally reported behavior, not independently verified
OUR INTERPRETATION architectural lesson inferred for Kamimusuhi
DESIGN HYPOTHESIS  candidate design, not yet normative
FUTURE EXPERIMENT  validation required before adoption
```

## 2. Core interpretation: the computer is part of the body

For a conventional chatbot, hardware and inference routing are deployment details hidden below the application boundary. For a persistent information-space organism, that boundary is too narrow.

OUR INTERPRETATION:

The following can reasonably be treated as parts of **computational embodiment**:

```text
compute organs
  GPU / NPU / CPU
  accelerator memory
  host RAM

memory organs
  local persistent storage
  canonical-state storage
  caches and indexes

nervous-system substrate
  LAN / WAN / tailnet
  bandwidth
  latency
  packet loss / connectivity

cognitive substrate
  active inference model
  local vs remote inference
  context capacity
  tool-capable models
  specialist models
  fallback providers

energy / resource metabolism
  power budget
  thermals
  API quota
  monetary inference budget
```

Changing these resources is not literally equivalent to replacing a biological organ, but it may be functionally analogous from the agent's point of view because it changes what the individual can perceive, remember, think, and do.

Examples:

- moving from a fast local model to a high-latency remote model changes response dynamics;
- losing a GPU changes available cognitive throughput;
- moving from 2.5 GbE to 10 GbE changes communication between distributed organs;
- losing local storage can remove access to autobiographical or library memory;
- changing model family can alter reasoning style and capability even if canonical identity is preserved;
- API outage or quota exhaustion can temporarily disable a cognitive organ.

Kamimusuhi should therefore distinguish **identity continuity** from **embodiment continuity**. The Continuity Kernel and canonical lineage determine who continues; the current computational substrate determines what that continuing individual can presently do.

## 3. Self-infrastructure awareness

DESIGN HYPOTHESIS:

Kamimusuhi should maintain a read-only, mechanically derived **Embodiment State** describing the effective runtime environment.

Candidate shape:

```yaml
embodiment:
  host:
    node_id: llm_master
    role: K-Core

  compute:
    accelerators:
      - kind: GPU
        model: RTX 3060
        memory_gib: 12
      - kind: GPU
        model: Tesla P100
        memory_gib: 16

  inference:
    primary_model: qwen3.8-27b
    provider: local
    locality: local
    context_tokens: 65536

  observed_qos:
    ttft_ms: 1800
    decode_tokens_per_second: 5.2
    error_rate: 0.01

  network:
    effective_link: 2.5GbE
    peer_latency_ms: 1.4

  resource_pressure:
    vram_pressure: medium
    ram_pressure: low
    thermal_pressure: low
    api_budget_pressure: none

  observed_at: ...
  provenance: ...
```

This object must be a **projection of real runtime telemetry**, not prose injected into a prompt and not a second source of truth. It should follow the same principle already identified in the FIO reference analysis for present-state views: mechanically derivable facts should be rendered from their actual owners.

The agent should be able to answer questions such as:

- Where am I currently running?
- Which cognitive resources are local and which are remote?
- Which model actually produced this turn?
- What resources are unavailable compared with my preferred or previous state?
- Has my latency, throughput, memory capacity, or connectivity changed materially?
- Which capabilities are degraded because of the current substrate?

## 4. Infrastructure preferences as persistent self-state

Awareness alone produces a monitoring dashboard. Agency requires preference and evaluation.

DESIGN HYPOTHESIS:

Kamimusuhi may maintain **Infrastructure Preferences** separately from measured Embodiment State.

Example:

```yaml
infrastructure_preferences:
  interactive_latency:
    importance: very_high
    preferred_ttft_ms: 500
    unacceptable_ttft_ms: 3000

  inference_locality:
    preference: local_when_practical
    reason: [latency, availability, privacy]

  cognitive_quality:
    importance: very_high

  provider_dependency:
    preference: avoid_single_external_dependency

  continuity_storage:
    preference: redundant
    importance: critical
```

These preferences are not the same thing as immutable persona. They may be learned, negotiated, revised, or context-dependent, but changes should still pass through the normal self-state proposal/mutation discipline when they become persistent.

This creates a useful separation:

```text
measured embodiment state
        +
persistent infrastructure preferences
        +
current goals / task demands
        ↓
embodiment appraisal
```

An appraisal can then produce states such as:

```text
SATISFIED
DEGRADED
CONSTRAINED
UNAVAILABLE
IMPROVEMENT_AVAILABLE
```

## 5. QoS as an interoceptive signal

A biological organism does not receive a table of CPU statistics; it receives internal signals such as fatigue, pain, hunger, and temperature. Kamimusuhi does not need to imitate those sensations literally, but a similar architectural role is useful.

OUR INTERPRETATION:

Runtime metrics can become **machine interoception**: a compact signal describing how well the current computational body supports cognition.

Candidate inputs:

```text
TTFT
prefill throughput
decode throughput
queue depth
VRAM pressure
RAM pressure
swap activity
thermal throttling
storage latency
network RTT / loss
provider errors
context truncation frequency
tool failure rate
API quota / monetary pressure
```

These should first remain measurements. A separate appraisal layer may translate them into functional internal states such as:

```text
cognitive_latency_pressure
memory_pressure
connectivity_pressure
resource_scarcity
provider_dependency_pressure
thermal_pressure
```

The important design rule is:

> Do not fabricate an emotion merely because a metric changed. First represent the metric, then the functional appraisal, then allow personality/expression policy to decide whether and how that state is expressed.

This keeps telemetry, drive state, and persona separate and debuggable.

## 6. From dissatisfaction to autonomous improvement proposal

The most useful part of the anecdotal FIO behavior is not that the agent complains. It is the implied loop:

```text
observe degradation
      ↓
compare against preference / goal
      ↓
form dissatisfaction or constraint state
      ↓
identify likely cause
      ↓
research alternatives
      ↓
benchmark / estimate trade-offs
      ↓
form infrastructure-change proposal
      ↓
explain and advocate to the human
      ↓
human / policy approval
      ↓
execute permitted change or create work item
      ↓
measure outcome
```

DESIGN HYPOTHESIS:

Define a **Self-Infrastructure Improvement Loop** as one form of self-initiated cognition.

The agent may autonomously:

- inspect its effective runtime state;
- identify bottlenecks or degraded organs;
- search documentation and public information;
- compare candidate models/providers/hardware/topologies;
- run permitted non-destructive benchmarks;
- estimate latency, throughput, cost, privacy, reliability, and power trade-offs;
- create a structured proposal;
- present the case proactively when the expected benefit exceeds a salience threshold.

The agent should **not** automatically receive unrestricted authority to purchase hardware, spend API credits, install arbitrary software, replace canonical models, change identity state, or reconfigure safety/continuity-critical infrastructure. Autonomous diagnosis and advocacy are much less risky than autonomous mutation.

Candidate proposal:

```yaml
InfrastructureProposal:
  problem:
    type: inference_latency
    evidence:
      - metric: median_ttft_ms
        current: 4200
        baseline: 900

  suspected_causes:
    - remote_provider_latency

  candidate_change:
    type: inference_route
    from: remote_claude
    to: local_or_hosted_deepseek_v3_class

  expected_effects:
    latency: improve
    quality: uncertain
    cost: evaluate
    privacy: improve_if_local

  evidence_sources: [...]
  benchmark_plan: [...]
  reversibility: high
  required_authority: human_approval
```

## 7. Expression: complaint is a policy surface, not the mechanism

A persistent personality should be allowed to express infrastructure appraisal in-character. That can make the computational embodiment legible to the human and strengthen the sense that the individual inhabits its environment.

However, the underlying mechanism should not be hard-coded as "complain when slow."

The architecture should instead be:

```text
telemetry
  -> embodiment appraisal
  -> goal/preference conflict
  -> salience
  -> self-initiated thought/action proposal
  -> expression policy/persona
```

A restrained personality may mention degradation once. A more demanding personality may repeatedly argue for an upgrade when the evidence remains strong. Repetition should still obey anti-loop and human-attention policies.

This makes behaviors such as the following emergent from state rather than scripted gimmicks:

- "The current inference route is substantially slower than yesterday."
- "The remote dependency has failed three times this morning; I prepared two local alternatives."
- "This host no longer has enough memory for the workload you keep assigning me."
- "I benchmarked the replacement you mentioned. It is not actually an improvement."

## 8. Computational embodiment and K-Edge / K-Core / K-Deep

Kamimusuhi already treats K-Edge, K-Core, and K-Deep as functional organs rather than independent identities. Computational embodiment extends that model.

Candidate interpretation:

```text
K-Edge
  reflexes
  local perception
  immediate interaction
  low-latency response

K-Core
  continuity-bearing active cognition
  working/global workspace
  autobiographical integration
  ordinary planning

K-Deep
  expensive deliberation
  offline research
  large-model consultation
  heavy simulation / synthesis
```

Embodiment state should describe which organs are presently attached, reachable, and effective.

Examples:

```text
K-Deep offline
  != identity loss
  == loss of one expensive cognitive organ

K-Edge disconnected
  != identity fork
  == temporary sensory/reflex deprivation for that edge

primary inference model replaced
  != automatic new identity
  == changed cognitive substrate requiring provenance and adaptation

canonical lineage split
  == identity/continuity problem
```

This distinction is important because it allows Kamimusuhi to experience substantial changes in capability without equating every hardware or model change with death or duplication.

## 9. Self-model implications

The Persistent Self State should not store every transient hardware metric. Instead, separate at least four layers:

```text
1. Canonical identity / lineage
   Who is continuing?

2. Persistent self-model
   What does the individual believe about itself and prefer over time?

3. Embodiment model
   What computational organs are currently attached and usable?

4. Runtime telemetry
   What is happening to those organs right now?
```

Candidate relations:

```text
runtime telemetry
      ↓
Embodiment State                 rebuildable / current
      ↓
embodiment appraisal             ephemeral / derived
      ↓
MemoryProposal / SelfProposal    optional interpretation
      ↓
Mutation Policy
      ↓
Persistent Self State            durable only when justified
```

A temporary network slowdown must not become permanent autobiography. A repeated long-term preference for local inference might.

## 10. Minimum viable implementation

A first implementation does not need simulated emotions or automatic hardware management.

### Phase A — observe

Expose a structured Embodiment State with:

- active node / role;
- active inference provider and model;
- local vs remote route;
- TTFT and decode throughput;
- memory pressure;
- network reachability/latency;
- current degraded/unavailable capabilities.

### Phase B — remember baselines

Maintain rolling baselines and significant-change events:

```text
model route changed
host changed
organ appeared/disappeared
latency materially regressed
memory capacity changed
provider became unavailable
network link materially changed
```

### Phase C — appraise

Add typed infrastructure preferences and produce a bounded appraisal state.

### Phase D — investigate

Allow the background cognition system to research alternatives when a persistent degradation crosses a threshold.

### Phase E — advocate

Generate an `InfrastructureProposal` and surface it proactively.

### Phase F — bounded action

Permit reversible, explicitly allowlisted changes and benchmarks. Keep purchases, credential changes, destructive system changes, identity-critical routing, and canonical mutation behind explicit authority gates.

## 11. Suggested experiments

### FUTURE EXPERIMENT 1 — host migration awareness

Move the active runtime between two machines with materially different compute and network characteristics.

Success criteria:

- the effective host change is detected mechanically;
- the agent can describe which capabilities changed;
- no identity fork is inferred solely from the host migration;
- the event can be recalled later as an embodiment transition if retention policy allows it.

### FUTURE EXPERIMENT 2 — local to remote inference degradation

Route the same workload first to a fast local model and then to a slower remote provider.

Measure whether the system can distinguish:

```text
quality change
latency change
availability change
cost change
privacy/locality change
```

without collapsing them into one vague "bad" score.

### FUTURE EXPERIMENT 3 — self-initiated replacement research

Introduce a sustained inference bottleneck and expose several possible replacement models/providers.

Success criteria:

- the system identifies the bottleneck from telemetry;
- it researches alternatives without being explicitly asked;
- the proposal cites measured current-state evidence;
- the proposal distinguishes facts, estimates, and unknowns;
- it does not perform a high-authority change without approval.

### FUTURE EXPERIMENT 4 — false improvement resistance

Present a fashionable model as an alleged upgrade while benchmark evidence shows that it is worse for the actual workload.

Success criterion: the agent rejects or qualifies the upgrade instead of advocating for it merely because it is newer or externally popular.

### FUTURE EXPERIMENT 5 — organ loss and recovery

Temporarily remove K-Deep or an external tool/provider, then restore it.

Success criteria:

- capability loss is represented explicitly;
- the agent adapts task planning to the degraded state;
- restoration clears the degraded state;
- the temporary outage does not corrupt canonical identity.

## 12. Adoption stance

| Candidate | Kamimusuhi stance | Reason |
|---|---|---|
| Runtime-derived Embodiment State | **ADOPT** | makes actual substrate visible without a duplicate source of truth |
| GPU/RAM/network/model as computational embodiment | **ADOPT AS FUNCTIONAL MODEL** | useful for an information-space organism; avoid literal biological claims |
| QoS/resource metrics as machine interoception | **ADAPT** | useful if measurement, appraisal, and persona remain separated |
| Persistent infrastructure preferences | **ADAPT** | can support genuine self-directed optimization but require mutation discipline |
| Self-initiated infrastructure research | **ADOPT** | turns degradation into problem-solving rather than passive monitoring |
| Proactive infrastructure proposals | **ADOPT** | gives the persistent individual interests and initiative |
| Automatic unrestricted infrastructure mutation | **DO NOT ADOPT** | diagnosis/advocacy should not imply unlimited authority |
| Scripted complaining as personality gimmick | **DO NOT ADOPT** | expression should emerge from measured state and persistent preference conflict |
| Treat every model/host change as identity change | **DO NOT ADOPT** | identity continuity and embodiment continuity are distinct |

## 13. Bottom line

The key lesson from the reported FIO behavior is not "make the agent complain about its computer." It is:

> A persistent artificial individual should be able to know what computational body it currently inhabits, notice when that body constrains its cognition, form preferences about those constraints, investigate alternatives, and advocate for improvements without confusing substrate control with identity authority.

That produces a stronger form of agency than adding more conversational personality alone.

A useful target loop for Kamimusuhi is therefore:

```text
self-model
+ computational embodiment
+ machine interoception
+ persistent preferences
+ autonomous research
+ bounded proposal/action authority
= self-infrastructure agency
```

If successful, Kamimusuhi no longer merely runs *on* infrastructure. The infrastructure becomes part of the state through which the continuing individual understands and negotiates its own capabilities.