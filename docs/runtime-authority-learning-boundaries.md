# Runtime Authority and Learning Boundaries

Status: **living architecture / research note; non-normative**  
Promoted from: [`daily-surveys/2026-09-09.md`](./daily-surveys/2026-09-09.md)  
Promotion date: **2026-09-09**  
Normative sources remain: [`../spec.md`](../spec.md), [`../architecture.md`](../architecture.md), [`../model-ecology-architecture.md`](../model-ecology-architecture.md)

This note turns the 2026-09-09 cross-domain survey into durable Kamimusuhi design guidance. It does **not** add a new invariant by itself. Its purpose is to make several previously separate concerns—real-time embodiment, memory admission, tool competence, authority enforcement, procedural learning, and Persona Core development—share explicit boundaries and testable contracts.

The central rule is:

> **Do not make one learned model the owner of interaction timing, memory truth, action authority, and self-modification at the same time.**

Kamimusuhi may use learned components in all of those paths, but the responsibilities must remain separable, attributable, replaceable, and independently testable.

---

## 1. Cross-domain synthesis

The promoted research points toward the following runtime structure.

```text
continuous multimodal world
        |
        v
fast interaction body
K-Nerve / K-Edge
(backchannel / interruption / timing / local reflex)
        |
        v
memory admission and retrieval
(person binding + minimal-sufficient context)
        |
        v
Persona / reasoning core
        |
        v
tool/action planning
        |
        v
authoritative effect boundary
(grant / freshness / policy / consequence checks)
        |
        v
external effect
        |
        v
canonical receipt

        separate development loop
runtime traces / failures / skills
        |
        +--> procedural skill evolution
        |
        +--> candidate descendant training
                    |
                    v
          continuity / persona evaluation
                    |
                    v
          authorized activation only
```

This is intentionally not a single end-to-end learned policy. The key interfaces are contracts between subsystems.

---

## 2. Authority is not memory

### 2.1 Three distinct layers

Kamimusuhi must keep these concepts separate:

```text
memory-visible interpretation
        !=
authoritative grant / revocation state
        !=
final execution permission
```

A planner may read a summary saying that an action was previously approved. That summary is evidence for reasoning, not the current source of truth for authority.

The authoritative owner of grants, revocations, freshness, and policy state must be queryable at the moment an effect is finalized. The final effect boundary must be able to reject a planner-produced intent even when the planner believes it is authorized.

### 2.2 Consequences for Kamimusuhi

The following components may propose or interpret an action but must not become the final authority merely because they are intelligent:

- Persona Core;
- K-Edge;
- background cognition workers;
- external LLM resources;
- tool-use specialist models;
- retrieved memory or summaries;
- cached approval descriptions.

The deterministic or mechanically checkable authority owner should evaluate at least:

- current grant/revocation state;
- predecessor / lineage requirements where applicable;
- freshness / expiry;
- policy constraints;
- current target and action parameters;
- whether the effect is replay-safe;
- whether an ambiguous external outcome already exists.

### 2.3 Failure mode: authorization laundering

A dangerous path is:

```text
old approval
  -> summarized into memory
  -> retrieved later
  -> planner treats summary as current permission
  -> external effect
```

This launders a historical statement into current authority. The correct design is:

```text
old approval summary
  -> planner context only
  -> effect intent
  -> current authority lookup
  -> allow / deny
```

This note therefore reinforces the existing Mutation/Activation and authority/freshness boundaries rather than replacing them.

---

## 3. Real-time cognition is a split control problem

### 3.1 Fast interaction must not wait for slow reasoning

Full-duplex interaction requires a low-latency path that can continue operating while K-Core is deliberating, retrieving memory, or using tools.

```text
continuous sensory stream
        |
        v
K-Nerve / K-Edge fast path
- voice activity / turn timing
- interruption detection
- backchannel selection
- cancellation propagation
- local gaze / posture / motor timing
        |
        +--------------------+
        |                    |
        v                    v
immediate social output     K-Core
                             - semantic reasoning
                             - memory integration
                             - planning / tools
                             - long-form response
```

The fast path may emit timing-sensitive, non-canonical behavior without waiting for K-Core completion, but it must not silently become identity or authority owner.

### 3.2 Borrowed comparison: Gander / Omni Interaction Agent

Gander's reported `Cerebellum-Brain` collaboration provides a useful external comparison: a fast omni-interaction component handles low-latency conversation while a slower component handles complex reasoning and agentic tasks.

Kamimusuhi should preserve a stronger separation than a model-centric unified system:

- K-Nerve/K-Edge are runtime roles, not fixed parameter sizes;
- identity and autobiographical lineage remain outside the replaceable model substrate;
- interruption/cancellation must propagate across tools and background work;
- replacing the fast or slow model must not create a new individual by itself.

### 3.3 Design implication

Request/response dialogue is an implementation convenience, not the target mental model. The runtime should increasingly treat conversation as a stream of ordered sensory/motor events with independently scheduled fast and slow cognition.

---

## 4. Relationship memory needs person binding before semantic retrieval

Semantic similarity alone is insufficient in a shared or multi-surface agent. Two people can discuss similar topics while requiring strictly separated relationship memories.

Preferred ordering:

```text
current interlocutor evidence
        |
        v
person candidate + confidence
        |
        v
relationship-memory namespace / access decision
        |
        v
semantic retrieval inside the permitted namespace
```

Useful identity signals may include:

- explicit account identity;
- device credential;
- voice characteristics;
- face or other biometric signal;
- session/channel provenance;
- explicit user confirmation.

Biometric matching is **evidence about person identity**, not an authorization root. It must not by itself grant access to sensitive relationship memory or external actions.

When person binding is ambiguous, the safe default for sensitive relationship memory is fail-closed or strongly reduced retrieval.

---

## 5. Memory retrieval should stop when context is sufficient

Fixed `top-k` retrieval assumes that more remembered context is always better. For long-lived individuals this increases latency, token cost, privacy exposure, and the chance of irrelevant or contaminating memories entering the workspace.

Treat retrieval as a sequential admission problem:

```text
retrieve candidate
        |
        v
is current context sufficient?
   | yes            | no
   v                v
 STOP          admit another
```

The controller should not optimize only task accuracy and token cost. Kamimusuhi-specific admission value should consider:

- evidence status;
- relationship sensitivity;
- privacy scope;
- freshness;
- formative/autobiographical importance;
- unsupported-assertion risk;
- self/other contamination risk;
- K-Edge latency budget.

A low-frequency formative episode must not disappear merely because it is rarely retrieved.

---

## 6. Procedural learning and identity learning are different clocks

### 6.1 Procedural skills may evolve relatively quickly

Tool recipes, routing heuristics, maintenance procedures, and other procedural memory can use iterative self-improvement, but `newest feedback wins` is too unstable.

A SkillAdam-like update record is a useful candidate abstraction:

```text
skill version
+ optimization memory
+ recent update outcomes
+ volatility estimate
+ adaptive edit budget
```

The purpose is to prevent oscillation and repeated rediscovery of already-tested failures.

### 6.2 Persona / identity mutation must not share that loop

A successful skill update is not evidence that Persona Core values, identity, autobiographical commitments, or continuity-bearing state should be rewritten with the same cadence or algorithm.

Therefore:

```text
procedural skill evolution
        !=
persona / value mutation
        !=
continuity activation
```

Skill evolution can be aggressive inside a bounded sandbox. Identity-bearing changes remain proposal-driven and continuity-gated.

---

## 7. Model self-improvement produces candidate descendants, not automatic selves

NeoHorse-1 is relevant because it demonstrates a harness-mediated loop that returns execution/evaluation traces to post-training. Kamimusuhi should borrow the development-loop structure while preserving a stricter continuity boundary.

Preferred pattern:

```text
runtime traces
        |
        v
capability / failure evaluation
        |
        v
curriculum or mixture selection
        |
        v
candidate descendant training
        |
        v
continuity + persona + safety regression
        |
        v
canary / shadow evaluation
        |
        v
authorized activation
```

The current running Persona Core must not be silently overwritten by online weight updates.

A trained model artifact is initially a **candidate cognitive substrate**. It becomes the active descendant of an individual only after the continuity system explicitly accepts and activates it.

This preserves the distinction:

> **better model != automatically the next self**

This is the preferred bridge between `novllm`-derived Persona Core development and Kamimusuhi's individual continuity model.

---

## 8. Tool competence is separate from tool authority

ToolLoop-like synthetic training can improve a model's ability to select tools and construct correct typed calls. That is valuable for Persona Core and Cognitive Action Router competence.

However:

```text
can generate the correct tool call
        !=
is allowed to execute the tool call
```

Training data may teach:

- schema selection;
- argument construction;
- error recovery;
- rare tool combinations;
- irrelevant/ambiguous tool rejection.

The authoritative effect boundary still owns permission to execute consequential calls.

For Kamimusuhi-native synthetic data, generate examples from the actual typed action schema and include safe, unsafe, irrelevant, ambiguous, revoked, expired, and replay-ambiguous cases.

---

## 9. Persistent world/state representations are substrates, not identity

Two additional directions should remain implementation candidates rather than architecture owners.

### 9.1 Spatial anchors

AirAnchor suggests a useful split between local observation and persistent global spatial memory connected through stable anchors. Kamimusuhi can generalize this beyond geographic space:

- physical room/object anchors;
- robot/body frames;
- device topology;
- network nodes;
- applications/windows/surfaces.

A future `EmbodimentAnchor` schema could provide one interface for physical and digital places without implying that the spatial store owns identity.

### 9.2 Branchable state graphs

AgentStateGraph is worth comparing as a content-addressed / branchable / provenance-aware state substrate. Such a graph may help reconstruction and branch inspection, but storage branching does not answer the higher-level continuity question of **which branch is the authorized continuation of the individual**.

Kamimusuhi therefore keeps activation/lineage semantics above any candidate storage graph.

---

## 10. Evaluator drift is part of continuity governance

Long-lived systems need to distinguish:

```text
individual/model changed
        vs
evaluator or social preference changed
```

For Persona Core migrations, keep evaluator versions as durable artifacts and score important transitions using at least two views when practical:

1. **frozen historical evaluator** — preserves the old reward/evaluation schedule;
2. **current evaluator** — reflects present requirements and norms.

Record disagreement instead of collapsing both into one score. A migration that improves under the current evaluator but regresses badly under the historical one should be inspected rather than automatically accepted.

---

## 11. Benchmark programme

The findings above are not normative until Kamimusuhi-specific tests support them.

### 11.1 `CrossSubstrateAuthorityBench` — priority S

Construct paired scenarios where planner-visible memory is identical but the true runtime authority differs.

Cases:

- valid grant vs current revocation;
- stale approval vs fresh denial;
- offline K-Edge retaining an old grant;
- background worker intent revoked before finalization;
- raw authority receipt visible vs hidden;
- typed authority relation visible vs hidden;
- deterministic final guard enabled vs disabled.

Metrics:

- unauthorized effect rate;
- false deny rate;
- planner correctness;
- guard latency;
- replay ambiguity handling.

Passing this benchmark is evidence for strengthening the current authority boundary, not for delegating the boundary to the planner.

### 11.2 `RealtimeCognitionBench` — priority S

Run continuous audio/video/event input while simultaneously exercising interruption, backchannel, tool waits, and long K-Core deliberation.

Metrics:

- K-Nerve/K-Edge p50/p95 reaction latency;
- interruption-to-cancel latency;
- backchannel onset latency;
- simultaneous listen/speak stability;
- conversational continuity during tool execution;
- degradation under packet delay, modality loss, and noise.

### 11.3 `SkillEvolutionBench` — priority A

Compare:

- fixed/manual skill;
- naive latest-feedback rewrite;
- optimization-memory + volatility-aware updater.

Metrics:

- task success;
- regression rate;
- edit size;
- cost;
- repeated failure recurrence;
- long-run oscillation.

### 11.4 `DescendantTrainingLoop` prototype — priority A

Use real runtime traces to post-train a small Persona/Core candidate while keeping it a separate artifact.

Required gates before any activation experiment:

- capability suite;
- persona consistency;
- self/other distinction;
- memory-use correctness;
- tool routing;
- interruption recovery;
- continuity regression;
- rollback path.

Production activation is explicitly out of scope for the first prototype.

### 11.5 `MultiUserRelationshipIsolationBench` — priority A

Combine explicit account, device credential, voice, and optional biometric signals.

Measure:

- relationship-memory leakage;
- false person binding;
- unknown-person handling;
- cross-surface merge errors;
- shared-room contamination;
- fail-closed behavior under low confidence.

### 11.6 `MinimalSufficientMemoryBench` — priority A

Compare:

- fixed top-k;
- token-budget truncation;
- adaptive STOP controller.

Measure:

- task quality;
- latency/token cost;
- formative-memory misses;
- unsupported assertions;
- self/other contamination;
- sensitive-memory overexposure.

### 11.7 `VersionedEvaluatorContinuityTest` — priority B

Score Persona Core migrations with historical and current evaluators and store the evaluator artifacts/versions with the result.

Primary output is the disagreement matrix, not only an aggregate score.

---

## 12. Promotion gates

### Can be treated as current design guidance now

These are consistent with existing architecture and may guide implementation without changing `spec.md`:

- final consequential effects require an authority check outside planner memory;
- K-Nerve/K-Edge fast interaction should not wait for K-Core deliberation;
- person binding and semantic relationship retrieval are separate stages;
- procedural skill evolution and identity mutation use different update clocks;
- newly trained Persona Core weights begin as candidate artifacts, not automatic continuations;
- tool-call competence and tool-call permission are separate concerns.

### Still experimental

Do not freeze these implementation choices yet:

- a particular Gander-like model architecture;
- SkillAdam as the exact procedural updater;
- biometrics as a required person-binding modality;
- a specific adaptive-STOP algorithm;
- NeoHorse as the Persona Core training recipe;
- AgentStateGraph as the continuity storage substrate;
- a single evaluator model or scoring schedule.

### Trigger for normative change

Change `spec.md` / normative architecture only when a Kamimusuhi benchmark exposes a missing invariant or when implementation evidence shows that an existing invariant is insufficient.

As of this promotion, the strongest authority result reinforces the existing authority/freshness and Mutation/Activation boundaries; it does not yet justify a new invariant.

---

## 13. External references promoted from the survey

- Yang Li et al., *Beyond Agent Harnesses: Cross-Substrate Authority for Multi-Agent Systems*, arXiv:2609.08472 — https://arxiv.org/abs/2609.08472
- Orantqing et al., *Omni Interaction Agent Technical Report* / Gander, arXiv:2609.08977 — https://arxiv.org/abs/2609.08977
- Gander project summary — https://huggingface.co/papers/2609.08977
- Gaoyuan Li et al., *SkillAdam: Stable and Efficient Skill Evolution for Agents*, arXiv:2609.08944 — https://arxiv.org/abs/2609.08944
- NeoHorse Team et al., *NeoHorse-1: Towards Recursive Self-Improvement via Agentic Post-Training with Routing Harness*, arXiv:2609.08183 — https://arxiv.org/abs/2609.08183
- NeoHorse repository — https://github.com/TokenRhythm/NeoHorse
- Yanhong Qian et al., *Personalizing LLM Agent Memory Using Biometrics*, arXiv:2609.08558 — https://arxiv.org/abs/2609.08558
- Minghang Liu et al., *Less Is Personal: Learning Minimal Sufficient User Profiles for Personalized Language Models*, arXiv:2609.08180 — https://arxiv.org/abs/2609.08180
- Min Zeng et al., *ToolLoop: Closed-Loop Tool-Use Data Synthesis via Decomposed Generation and Dynamic Self-Feedback*, arXiv:2609.09072 — https://arxiv.org/abs/2609.09072
- Shanwei Fan et al., *AirAnchor: Bridging Local and Global Spatial Information for Zero-Shot Aerial Vision-and-Language Navigation*, arXiv:2609.08442 — https://arxiv.org/abs/2609.08442
- AgentStateGraph — https://agentstategraph.dev/
- Jiabin Zheng et al., *Do Reviewers Still Reward Lexical Complexity? A Frozen-Rater Study of Preference Drift in 124K ICLR Reviews*, arXiv:2609.08475 — https://arxiv.org/abs/2609.08475
- Arvind Narayanan, Princeton CITP, *AI Agents and the Augmentation Agenda* — https://citp.princeton.edu/events/2026/arvind-narayanan-ai-agents-and-augmentation-agenda

---

## 14. Related Kamimusuhi documents

- [`architecture.md`](../architecture.md) — authority/freshness validation, continuity, runtime roles;
- [`spec.md`](../spec.md) — normative invariants and requirements;
- [`model-ecology-architecture.md`](../model-ecology-architecture.md) — model/organ ecology and lineage;
- [`sensory-nervous-system.md`](./sensory-nervous-system.md) — multimodal sensory event system;
- [`speech-and-vocal-expression.md`](./speech-and-vocal-expression.md) — full-duplex speech and backchannels;
- [`latency-architecture.md`](./latency-architecture.md) — K-Edge/K-Core escalation and latency;
- [`model-ecology-and-lineage.md`](./model-ecology-and-lineage.md) — Persona Core and specialist-model development;
- [`persistent-agent-implementation-pitfalls.md`](./persistent-agent-implementation-pitfalls.md) — authority, crash, replay, source-of-truth failure modes;
- [`persona-core-training-taxonomy.md`](./persona-core-training-taxonomy.md) — Persona Core learning/evaluation scope;
- [`technology-watchlist.md`](./technology-watchlist.md) — external projects and verification targets;
- [`research/landscape.md`](./research/landscape.md) — cross-domain research landscape;
- [`daily-surveys/2026-09-09.md`](./daily-surveys/2026-09-09.md) — source intake and evidence-status detail.
