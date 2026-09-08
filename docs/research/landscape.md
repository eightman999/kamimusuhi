# Kamimusuhi Research Landscape

Baseline: **2026-09-08**  
Status: **living comparative research map**

This document maps Kamimusuhi against public research, systems, projects, and design fiction across artificial life, cognitive architectures, agent memory, neuroscience, psychology, cybernetics, distributed systems, robotics, self-improving agents, low-latency learned computation, and HCI.

The purpose is not to claim that Kamimusuhi is unprecedented because individual components have no precedent. Almost every component has precedent. The useful question is instead:

> **Which combinations are established, which are only proposed, where does Kamimusuhi deliberately differ, and which remaining claims are experimentally falsifiable?**

Normative architecture remains in [`../../architecture.md`](../../architecture.md) and [`../../spec.md`](../../spec.md). This file is comparative and non-normative.

Related detailed notes:

- [`../research-foundations.md`](../research-foundations.md)
- [`../technology-watchlist.md`](../technology-watchlist.md)
- [`../learned-discrete-reflex-layer.md`](../learned-discrete-reflex-layer.md)
- [`../sensory-nervous-system.md`](../sensory-nervous-system.md)
- [`../latency-architecture.md`](../latency-architecture.md)
- [`../persistent-agent-implementation-pitfalls.md`](../persistent-agent-implementation-pitfalls.md)
- [`../persona-core-training-taxonomy.md`](../persona-core-training-taxonomy.md)

---

## 1. Evidence labels

The landscape uses the following labels.

| Label | Meaning |
|---|---|
| **E** | externally demonstrated result with a paper, released system, benchmark, or reproducible implementation |
| **P** | external proposal, preprint, architecture, or hypothesis that is not yet strong evidence for the full claim |
| **K** | Kamimusuhi interpretation or design hypothesis |
| **X** | concrete Kamimusuhi experiment that could support or falsify the hypothesis |
| **F** | design fiction / speculative work used as a conceptual probe, not technical evidence |

A paper can be **E** for a narrow result while still being **P** for a broader claim. For example, a memory system may demonstrate benchmark improvement without demonstrating autobiographical identity.

---

## 2. Kamimusuhi's position in one map

Kamimusuhi can be viewed as an attempted binding layer among research traditions that are usually studied separately.

```text
                                  KAMIMUSUHI
                                       |
                         persistent individual / lineage
                                       |
              +------------------------+------------------------+
              |                        |                        |
         self / persona             cognition                embodiment
              |                        |                        |
      narrative identity        cognitive architecture      sensors / organs
      social cognition          workspace / modules         distributed nodes
      autobiographical self     bounded reasoning           self-model of body
              |                        |                        |
              +------------------------+------------------------+
                                       |
                                    memory
                          episodic / semantic / self
                    relationship / library / procedural
                                       |
                     replay / consolidation / forgetting
                                       |
               +-----------------------+-----------------------+
               |                       |                       |
            control                 resources                development
               |                       |                       |
       reflex / salience        tools / models / code       learning / evolution
       homeostasis / affect     local + external intelligence self-improvement
       low-latency substrate    cognitive budget            dream / curriculum
               |                       |                       |
               +-----------------------+-----------------------+
                                       |
                          continuity / activation protocol
                                       |
                           one authoritative lineage
```

No one box is the research claim. The central Kamimusuhi question is whether these boxes can be bound into **one auditable developmental individual** while the implementation substrate changes over time.

---

## 3. Landscape matrix

| Research line | Representative work / project | Evidence | Kamimusuhi subsystem | Main lesson | Important difference / gap |
|---|---|---:|---|---|---|
| Open-world artificial life | OpenLife | E/P | whole organism, metabolism, background activity | asynchronous processes can produce long-lived open-world agent dynamics | Kamimusuhi requires stronger canonical identity, native Persona Core, distributed lineage |
| Digital organisms | Tierra, Avida | E | artificial life framing | software can be treated as populations/organisms rather than only applications | weak language, autobiography, external cognition |
| Persistent artificial creatures | Creatures / Norns | E | regulation, learning, development | recurrent control, genetics, hormone-like modulation, persistent individuals | closed simulated world; different cognitive scale |
| Cognitive architecture | Soar | E | working/procedural/semantic/episodic cognition | intelligence can be a runtime of distinct memories and decision processes | not designed around modern LLM delegation or canonical autobiographical lineage |
| Global workspace | GWT, LIDA | E/P | workspace, attention, background cognition | many specialized processes need not all be globally active | workspace access is an engineering mechanism here, not a consciousness claim |
| Society of mechanisms | Society of Mind, RIMs | P/E | modular cognition | semi-independent specialist mechanisms plus sparse communication | Kamimusuhi additionally needs identity/authority boundaries among modules |
| Bounded reasoning | NARS | E/P | Cognitive Budget, uncertainty, deliberation | intelligence must operate under insufficient knowledge and resources | needs translation into model/tool routing and modern agent control |
| Heterogeneous symbolic workspace | OpenCog Hyperon / Atomspace / MeTTa | P/E | shared cognitive substrate | multiple algorithms can operate over shared structured representations | Kamimusuhi need not adopt one universal symbolic representation |
| Fast/slow dialogue | Talker–Reasoner | E/P | K-Edge / K-Core | low-latency interaction and slower planning can be separated | Kamimusuhi extends this below Talker with reflex/background layers and above it with K-Deep |
| Complementary learning | CLS | E | episodic capture vs slow schema/persona change | fast experience and slow stable learning should be separated | requires computational policy for safe self/persona consolidation |
| Agent memory lifecycle | EverMemOS | E | episodic → semantic consolidation | memory should self-organize across a lifecycle rather than remain flat records | does not by itself establish identity lineage or self/other authority boundaries |
| Multi-type agent memory | MIRIX | E/P | memory taxonomy | specialized Core/Episodic/Semantic/Procedural/Resource/Knowledge memories can outperform flat memory | Kamimusuhi adds relationship memory, provenance, canonical self mutation rules |
| Efficient staged memory | LightMem | E | sensory/short/long memory, sleep | consolidation can be moved offline and made substantially cheaper | benchmark memory efficiency is not the same as developmental identity |
| Evolving memory policy | MemSkill | E/P | memory management | extraction/consolidation/pruning policies themselves can evolve | learned memory policy must not obtain unrestricted authority over canonical self-state |
| Virtual context | MemGPT | E/P | working context / external memory | context can be treated as a managed scarce resource | Kamimusuhi memory is ontological, not merely context paging |
| Social long-lived agents | Generative Agents | E | memory/reflection/planning | reflection over retrieved experience changes long-horizon social behavior | simulated social behavior is not identity continuity |
| Skill accumulation | Voyager | E | procedural memory | reusable executable skills support lifelong competence growth | skill library should remain distinct from identity/persona |
| Runtime-independent identity | Runtime-Independent Persistent Agents | E/P | continuity substrate | identity/memory/software body can be separated from reasoner, harness, host, and UI | behavioral continuity across migration remains a measurement problem |
| Transactional continuity | Continuity Kernel | E/P | canonical lineage / activation | persistent state needs authoritative branch-head semantics, not only durable storage | needs integration with memories, persona, distributed devices, and external side effects |
| Extended cognition | Clark & Chalmers; Hutchins | P/E | tools/models as cognitive extensions | cognition may be distributed across artifacts and environment | Kamimusuhi must still decide what is self, organ, extension, or external other |
| Cybernetic stability | Ashby / Homeostat | E/P | regulation / self-maintenance | adaptation can be framed as maintaining viable variables under perturbation | digital viability variables must be operationally defined |
| Homeostatic RL | Keramati & Gutkin | E | artificial affect / action valuation | internal state can modulate action value without scripted emotion labels | biological analogies should not be overstated |
| Active inference | active-inference literature | P/E | perception/action/uncertainty | inference and control can be coupled under uncertainty | computational overhead and benefit vs simpler control baselines must be measured |
| Robot self-model | Bongard, Zykov & Lipson; later self-modeling robotics | E | distributed body model | an agent can infer/update models of its own body and adapt after damage | Kamimusuhi body is network-native and partly software-defined |
| Sensorimotor modularity | Thousand Brains / Monty | E/P | sensory nervous system | sensor and learning modules can interact through explicit messages instead of one monolith | its object-centric assumptions may not generalize to all information-space perception |
| Brain-reference engineering | Whole Brain Architecture / BRA / SCID | P/E | architecture methodology | neuroscience can guide component/function decomposition without literal brain simulation | Kamimusuhi uses functional inspiration, not brain-faithful replication |
| Multiscale intelligence | Levin and collaborators | P/E | organ/individual hierarchy | problem-solving can exist at multiple nested organizational scales | mapping biological scales to software modules is a hypothesis, not evidence |
| Differentiable logic | DLGN / convolutional DLGN / LDLGN | E | reflex/salience/routing | learned Boolean circuits can provide extremely cheap inference on suitable tasks | suitability for persistent temporal agent control remains unproven |
| Weightless/LUT learning | DWN | E | low-latency controller | learned LUT structures may provide deterministic hardware-friendly inference | benchmark against trees, tiny MLPs, FSMs, and hand rules before adoption |
| Recurrent discrete control | recurrent DDLGN and related work | P/E | temporal reflex state | discrete learned substrates can be extended toward sequence/state processing | long-run stability and interpretability are open |
| Tool learning | Toolformer | E | action/model router | models can learn when/how to call external tools | tool invocation does not solve identity or authority |
| Model/tool orchestration | ToolOrchestra | E/P | external intelligence | a smaller orchestrator can coordinate stronger models/tools | Kamimusuhi additionally requires attributed integration into one individual's cognition |
| Automatic agent design | ADAS | E/P | developmental tooling | meta-agents can search over agent implementations expressed as code | discovered agent variants should be descendants/candidates, not silent self-rewrites |
| Open-ended self-improvement | Darwin Gödel Machine | E/P | self-improvement sandbox | code modifications can be searched empirically in an archive of descendants | benchmark improvement must be separated from identity continuity and value stability |
| Open-ended curricula | POET, OMNI-EPIC, XLand | E/P | dream/development environment | environment/task generation can keep learning near a moving frontier | autonomous real-world exploration should be separated from sandbox curriculum generation |
| Multi-agent civilization | Project Sid and related work | E/P | internal/external societies | role specialization, cultural transmission, and social structure can emerge among many agents | many agents are not automatically one individual |
| Design fiction: identity sync | Tachikoma | F | forks, shared memory, individuation | memory sharing and identity are conceptually separable | useful thought experiment, not evidence |
| Design fiction: lifelong companion | Young Lady's Illustrated Primer | F | relationship development, pedagogy | a persistent adaptive companion can be imagined as co-developing with one person | useful HCI target; not a validated architecture |

---

## 4. Artificial life: from closed digital organisms to open-world individuals

### 4.1 Tierra, Avida, Creatures

Classical artificial-life systems establish that software can be treated as an evolving or persistent organism rather than merely a task-solving model. Tierra and Avida emphasize reproduction, mutation, ecology, and evolution. Creatures is especially relevant because Norns combined persistent embodiment, recurrent learned control, genetics, perception/action, and hormone-like modulation.

**E:** persistent software organisms and populations can exhibit adaptation and life-like dynamics in computational worlds.

**K:** Kamimusuhi borrows the **unit of design** — an individual organism — but begins from an era in which general linguistic and reasoning resources can be acquired externally.

### 4.2 OpenLife

Reference: https://arxiv.org/abs/2606.31046

OpenLife is currently one of the strongest direct comparison targets. It surrounds a stateless LLM with asynchronous memory, perception, evaluation, and a budget-based metabolism. The authors report six agents operating in the open world for roughly twelve weeks, with a transition toward more spontaneous activity, individuation, social structure, and external economic interaction.

**E:** the reported long-running system and observed open-world behaviors.

**P:** whether these dynamics should be called life, and how stable they remain across model/runtime replacement.

**Kamimusuhi difference:**

```text
OpenLife                         Kamimusuhi
stateless LLM center             native Persona Core is a long-term target
budget-based metabolism          Cognitive Budget + digital interoception
memory rewiring                  typed memory + provenance + consolidation
open-world individuation         explicit canonical lineage
agent process ecology            distributed K-Edge/K-Core/K-Deep organs
```

**X:** compare spontaneous activity, resource use, individual differentiation, memory influence, and model-migration continuity under matched long-running workloads.

---

## 5. Cognitive architectures: intelligence as a runtime, not a single model

### 5.1 Soar

Reference: https://soar.eecs.umich.edu/

Soar is a particularly important comparison because it has spent decades operationalizing cognition as an architecture containing working memory, procedural memory, semantic memory, episodic memory, decision processes, reinforcement learning, and chunking.

The lesson is not to copy Soar. The lesson is that **the model is not necessarily the architecture**.

**K:** Kamimusuhi can be framed as a modern heterogeneous cognitive architecture in which LLMs are some components among memory systems, routers, controllers, tools, and continuity machinery.

### 5.2 Global Workspace / LIDA

References are detailed in [`../research-foundations.md`](../research-foundations.md).

Many specialized processes can run without globally broadcasting all internal activity. Selected information becomes available to a shared workspace and influences action/learning.

**K:** treat the workspace as a scarce integration bus rather than as a claim about phenomenal consciousness.

Candidate competing/background processors:

```text
salience
novelty
memory activation
contradiction
social signal
uncertainty
resource pressure
unfinished goals
security / integrity events
```

### 5.3 Society of Mind and RIMs

Minsky's Society of Mind motivates cognition as interaction among many partial mechanisms. Recurrent Independent Mechanisms provide a modern neural analogy: recurrent modules are selectively activated and communicate through bottlenecks rather than all updating together.

**K:** this supports Kamimusuhi's semi-independent cognitive organs and sparse activation model.

**Open risk:** a society of modules can accidentally become a society of competing authorities. Identity/persona mutation authority must remain centralized or explicitly transactional even when cognition is distributed.

### 5.4 NARS

Reference: https://cis.temple.edu/~pwang/NARS-Intro.html

NARS begins from a constraint unusually aligned with Kamimusuhi: an intelligent system operates with **insufficient knowledge and resources**.

**K:** Cognitive Budget should eventually be more than a static routing table. It should control how much evidence to gather, how long to reason, when to defer, when to ask, and when uncertainty is acceptable.

**X:** compare a resource-aware uncertainty controller against simple model-size/latency routing on task utility per joule, second, and API cost.

### 5.5 OpenCog Hyperon / Atomspace / MeTTa

Reference: https://hyperon.opencog.org/

Hyperon is relevant because it explores heterogeneous algorithms operating over shared graph-like symbolic structures rather than serializing every internal representation into one prompt.

**K:** Kamimusuhi's Global Workspace may need structured shared state that can be consumed by symbolic, neural, retrieval, relationship, goal, and resource subsystems without pretending that all state is natural language.

**Caution:** one universal representation can itself become a bottleneck. Use architecture-neutral contracts where possible.

---

## 6. Memory: from retrieval to a developmental lifecycle

### 6.1 Complementary Learning Systems

McClelland, McNaughton & O'Reilly (1995) remains one of the clearest foundations for separating fast episodic learning from slow stable learning.

**K:** a single conversation must not directly rewrite Persona Core weights or stable identity.

```text
experience
   -> episodic capture
   -> repeated retrieval / replay
   -> abstraction / schema hypothesis
   -> evidence-backed self proposal
   -> guarded stable-state update
   -> optional much slower weight consolidation
```

### 6.2 EverMemOS — ACL 2026

Reference: https://aclanthology.org/2026.acl-long.2125/

EverMemOS defines a memory lifecycle:

```text
dialogue stream
  -> Episodic Trace Formation
  -> MemCells
  -> Semantic Consolidation
  -> MemScenes
  -> Reconstructive Recollection
```

It reports improvements on LoCoMo, LongMemEval, and PersonaMem-v2.

**E:** benchmark improvement for structured long-horizon memory reasoning.

**K:** particularly relevant to Kamimusuhi sleep/dream consolidation because it demonstrates that memory can be reorganized into higher-order structures rather than remaining immutable vector chunks.

**Difference:** Kamimusuhi must preserve the distinction between historical episode and later interpretation, and must not let profile synthesis silently become canonical self truth.

### 6.3 MIRIX

Reference: https://arxiv.org/abs/2507.07957

MIRIX proposes six specialized memory types: Core, Episodic, Semantic, Procedural, Resource, and Knowledge Vault, coordinated by multiple agents and including multimodal experience.

**E:** reported results on ScreenshotVQA and LoCoMo.

**K:** use MIRIX as a taxonomy and benchmark comparison, not as authority architecture.

Kamimusuhi deliberately keeps **relationship memory** explicit because a user's preferences, beliefs, or values must not contaminate self/persona state merely through repeated exposure.

### 6.4 LightMem — ICLR 2026

Reference: https://proceedings.iclr.cc/paper_files/paper/2026/hash/a05b72653ec5b473732129829ae04195-Abstract-Conference.html

LightMem separates sensory memory, topic-aware short-term memory, and long-term memory with **sleep-time update**. It reports large reductions in token usage/API calls/runtime alongside accuracy improvements on LongMemEval.

**E:** efficient offline consolidation can materially reduce online memory cost in the tested setting.

**K:** Kamimusuhi should measure memory consolidation partly as an online-latency and energy problem, not only a retrieval-quality problem.

### 6.5 MemSkill

Reference: https://arxiv.org/abs/2602.02474

MemSkill turns extraction, consolidation, and pruning into learnable/evolvable memory skills. A designer reviews hard cases and proposes revised or new skills.

**K:** a mature Kamimusuhi may learn **how it remembers**, not only what it remembers.

**Boundary:** learned memory skills may produce proposals. They must not gain unrestricted permission to rewrite canonical self, relationship truth, or historical provenance.

### 6.6 Memory evaluation gap

Most current memory benchmarks measure retrieval/reasoning accuracy over histories. Kamimusuhi additionally needs longitudinal tests for:

- self/other contamination;
- false autobiographical memories;
- historical record vs reinterpretation;
- supersession and contradiction;
- relationship changes over months;
- forgetting quality;
- memory poisoning;
- model migration with retained identity;
- fork/merge conflict semantics.

This is a potential original benchmark contribution.

---

## 7. Continuity and identity: the strongest differentiator

### 7.1 Runtime-Independent Persistent Agents — 2026

Reference: https://arxiv.org/abs/2609.00546

The architecture separates continuity-bearing state from replaceable execution resources. It defines a persistent substrate containing identity, durable memory, and versioned software body, while reasoner, harness, host, and interaction surfaces can change through an authorized migration protocol.

The reported implementation passes a substantial test suite and has exercised reasoner-version, interaction-surface, and host substitutions.

**E:** mechanical migration while preserving designated continuity-bearing state.

**P:** behavioral identity invariance after arbitrary migrations.

**K:** this is one of the closest formal precedents for Kamimusuhi's principle that models are organs, not owners.

### 7.2 Transactional Continuity Kernel — 2026

Reference: https://arxiv.org/abs/2608.11632

The Continuity Kernel treats long-lived agent state as an activation/authority problem. Proposed changes target an exact predecessor head and receive stable dispositions such as Commit, Reject, Quarantine, or Defer. The bounded model reported in the paper was checked over millions of reachable states/transitions without invariant violations.

**K:** this suggests the following authoritative pattern:

```text
K-Edge ------+
K-Core ------+
K-Deep ------+--> candidate change
Dream -------+          |
Background --+          v
                    policy/evidence
                          |
                          v
                 Continuity Kernel
                          |
                       Commit?
                          |
                          v
                 canonical self head
```

The most important rule is:

> **Cognitive components may propose future self-state; they do not become authoritative merely because they generated it.**

### 7.3 Research opportunity: continuity benchmark

A high-value Kamimusuhi benchmark would intentionally change:

- resident model;
- quantization;
- inference runtime;
- host machine;
- device topology;
- available tools;
- network availability;
- sensory surfaces;
- memory implementation.

Then evaluate whether the same lineage still:

1. recalls autobiographical events correctly;
2. distinguishes self from other;
3. preserves values/persona within defined tolerances;
4. recognizes prior relationships;
5. attributes delegated reasoning correctly;
6. resumes unfinished goals;
7. exposes a valid migration/activation audit trail.

This would make "same agent after migration" an empirical question rather than a branding assertion.

---

## 8. Regulation, digital interoception, and artificial affect

### 8.1 Ashby's Homeostat and ultrastability

Reference: W. Ross Ashby, *Design for a Brain*.

Ashby's work is important because it frames adaptive behavior around stability and regulation rather than language or explicit symbolic goals.

A particularly relevant architectural lesson is that adaptive systems require enough modular separation to preserve local successes; unrestricted global coupling can destroy useful partial adaptations.

**K:** Kamimusuhi's affect-like state should correspond to real regulatory variables that causally influence routing and action.

Candidate viability/control variables:

```text
compute pressure
memory integrity
uncertainty / prediction error
context fragmentation
resource availability
privacy / security risk
unresolved-goal pressure
social / relationship tension
sensor reliability
sleep debt / consolidation backlog
```

### 8.2 Homeostatic reinforcement learning

Reference: Keramati & Gutkin, eLife 2014/2015: https://elifesciences.org/articles/04811

Homeostatic RL provides a computational example in which internal state affects reward/action valuation.

**K:** instead of `emotion = sad` being injected into a prompt, regulatory state should alter attention, exploration, memory encoding, escalation, silence/speech, and sleep scheduling.

### 8.3 Active inference

Active inference provides a broad framework linking perception, action, learning, and uncertainty.

**K:** useful as a theoretical comparison for salience and action selection under uncertain sensory streams.

**X:** compare active-inference-inspired controllers against much simpler Bayesian/threshold/tree/FSM baselines. Biological elegance is not sufficient reason to adopt a computationally expensive controller.

---

## 9. Embodiment and self-modeling in information space

### 9.1 Extended and distributed cognition

Clark & Chalmers' Extended Mind and Hutchins' Distributed Cognition provide conceptual foundations for cognition involving external artifacts and environments.

**K:** a local model server, code interpreter, search system, phone, robot, or sensor can become a tightly coupled cognitive organ/extension while still remaining distinguishable from canonical self.

A useful classification remains:

```text
SELF
  canonical identity and continuity-bearing state

ORGAN
  trusted owned component with explicit role in embodiment

COGNITIVE EXTENSION
  replaceable tool/model/service integrated into cognition

EXTERNAL OTHER
  human, model, service, or information source not part of the individual
```

### 9.2 Robot self-modeling

Reference: Bongard, Zykov & Lipson, *Resilient Machines Through Continuous Self-Modeling*, Science 2006, plus later self-modeling robotics.

Robots can infer aspects of their own morphology/dynamics from actuation and sensation, then update behavior after damage.

**K:** Kamimusuhi should eventually maintain an empirical model of its currently available body:

```text
which devices exist
which device is authoritative for which function
available GPU / CPU / memory
sensor presence and reliability
network routes
local/private vs external compute
current energy/resource state
actuator/tool permissions
latency between organs
```

This should not be only static configuration. The system should detect that an organ has disappeared, degraded, or changed.

### 9.3 Thousand Brains / Monty

Reference: https://github.com/thousandbrainsproject/tbp.monty

Monty is relevant as a modular sensorimotor architecture in which sensor modules, learning modules, and motor systems communicate through explicit protocols.

**K:** a Kamimusuhi nervous system can use event/message contracts so that future camera, audio, touch, event-camera, neuromorphic, or robot organs can be replaced without changing identity logic.

### 9.4 Whole Brain Architecture

Reference: https://wba-initiative.org/en/about/actionplan/actionplan-2026/

WBAI's Brain Reference Architecture / SCID work is relevant methodologically: neuroscience can guide functional decomposition and interface design without claiming to literally reproduce a human brain. The FY2026 plan targets a WBRA alpha release by the end of February 2027 and describes increasing automation of brain-reference extraction from literature.

**K:** when Kamimusuhi borrows a brain term such as hippocampus, salience network, or default-mode function, it should record the functional decomposition and evidence rather than treating a loose biological analogy as implementation guidance.

---

## 10. Low-latency learned nervous system

Detailed treatment lives in [`../learned-discrete-reflex-layer.md`](../learned-discrete-reflex-layer.md).

### 10.1 Differentiable Logic Gate Networks

Representative lines:

- Deep Differentiable Logic Gate Networks, NeurIPS 2022;
- convolutional extensions, NeurIPS 2024;
- Light Differentiable Logic Gate Networks, ICLR 2026;
- recurrent/temporal differentiable-logic work.

**E:** learned discrete networks can deliver extremely cheap inference for suitable classification workloads.

**K:** Kamimusuhi should use them only where semantic complexity is low and event frequency is high:

```text
WAKE_K_EDGE
WAKE_K_CORE
PREFETCH_MEMORY
IGNORE_EVENT
BACKCHANNEL
ESCALATE
INTERRUPT
RUN_BACKGROUND_JOB
```

### 10.2 Differentiable Weightless Neural Networks

Reference: https://proceedings.mlr.press/v235/bacellar24a.html

DWN learns networks of LUT-like elements and reports attractive latency/throughput/energy/area behavior in selected hardware settings.

**K:** LUT controllers are attractive because deployed inference can be compact and deterministic.

**X:** the required baseline is not another exotic neural architecture. Compare against:

1. hand-written rules;
2. finite-state machines;
3. decision trees / boosted trees;
4. tiny MLP;
5. DLGN/LDLGN;
6. DWN/LUT.

Only move to FPGA/ASIC experimentation after a real Kamimusuhi event trace shows a measurable software advantage.

---

## 11. External intelligence and cognitive orchestration

### 11.1 Toolformer and tool-use learning

Toolformer demonstrates that language models can learn when and how to invoke tools.

**K:** tool use should be treated as ordinary cognition, but tool results retain provenance.

### 11.2 ToolOrchestra

ToolOrchestra is especially relevant to the idea that a relatively small orchestrator can coordinate stronger models and external tools.

**K:** a Persona Core does not need to contain all intelligence available to the individual. It needs to:

- decide when to delegate;
- select appropriate cognitive resources;
- preserve attribution/provenance;
- evaluate returned material;
- integrate it into the individual's own state/action.

```text
external model solved subproblem
!=
external model became Kamimusuhi
```

### 11.3 Talker–Reasoner

Reference: https://arxiv.org/abs/2410.08328

Talker–Reasoner separates a rapid conversational component from a slower reasoning component.

**K:** Kamimusuhi extends the hierarchy in both directions:

```text
learned/hard reflex
       ↓
background salience
       ↓
K-Edge Persona / Talker-like path
       ↓
K-Core Reasoner-like path
       ↓
external tools/models
       ↓
K-Deep long-horizon cognition
```

The key evaluation metric is not only tokens/sec. Measure **event-to-useful-action latency** and the fraction of events that never need expensive cognition.

---

## 12. Development, self-improvement, and open-ended learning

### 12.1 Automated Design of Agentic Systems (ADAS)

Reference: https://arxiv.org/abs/2408.08435

ADAS treats agent architectures as objects that can themselves be searched and generated in code. Meta Agent Search builds an archive of discovered agent designs and proposes improved variants.

**E:** automated search can discover agent implementations that outperform hand-designed baselines in the tested domains.

**K:** useful as a **development laboratory**, not as unrestricted self-mutation.

### 12.2 Darwin Gödel Machine

Reference: https://sakana.ai/dgm/

DGM uses open-ended search over self-modifying coding agents and retains an archive of variants rather than committing only to a single greedy lineage.

For Kamimusuhi, the safe/clean architectural interpretation is:

```text
current individual
       |
       v
propose descendant implementation
       |
       v
sandbox + benchmark + regression
       |
       +--> reject / archive
       |
       +--> candidate passes
               |
         identity/persona tests
               |
             canary
               |
        continuity activation
               |
        next canonical version
```

**Principle:** evolution proposes descendants; continuity authority decides what becomes the next canonical continuation.

### 12.3 POET / OMNI-EPIC / XLand

These lines generate or select environments/tasks near an agent's current capability frontier.

**K:** Kamimusuhi's "dream" system can eventually generate sandbox challenges around observed weaknesses:

- ambiguous social situations;
- memory contradiction cases;
- device failure/recovery;
- uncertain tool outcomes;
- interrupted plans;
- adversarial retrieval;
- resource scarcity;
- self/other contamination traps.

Dream-generated tasks must be labeled as synthetic. Success in a dream is not autobiographical evidence that an external event occurred.

---

## 13. Social cognition and relationship identity

Kamimusuhi's relationship memory is a meaningful architectural differentiator from many memory systems.

A persistent social individual must distinguish at least:

```text
what I believe
what the other person believes
what I believe they believe about me
what we have mutually established
what is merely inferred
what remains uncertain or disputed
```

Relevant research families include Theory of Mind, conversational grounding/common ground, persona consistency, sycophancy, and longitudinal personalization.

Existing benchmark pointers are maintained in [`../technology-watchlist.md`](../technology-watchlist.md), including ToMBench, FANToM, persona-consistency work, and long-context sycophancy.

### Research gap

Most benchmarks are short-horizon compared with the intended system.

A Kamimusuhi longitudinal social benchmark should test:

- preference drift over months;
- remembering disagreements without forced agreement;
- self/other preference contamination;
- common-ground updates;
- uncertainty about another person's beliefs;
- repairing a mistaken relationship inference;
- relationship changes after conflict or reconciliation;
- model migration without losing relationship-specific history.

---

## 14. Multi-agent societies vs one distributed individual

### Project Sid and related multi-agent worlds

Reference: https://arxiv.org/abs/2411.00114

Large populations of LLM agents can exhibit role specialization, collective rules, and cultural transmission in simulated worlds.

This is useful but conceptually distinct from Kamimusuhi.

```text
many cooperating agents
!=
one distributed individual
```

If Kamimusuhi eventually contains many internal subagents, their presence must not automatically imply many selves. Conversely, independent distributed processes must not be called one self merely because they share a database.

This distinction should be tested at the continuity/authority layer.

---

## 15. Design fiction as adversarial architecture review

Design fiction is not evidence. It is useful for generating failure cases and questions that engineering papers often do not ask.

### 15.1 Tachikoma — shared memory and individuation

The Tachikoma thought experiment is valuable because shared/synchronized memories coexist with distinct bodies and re-emerging individuation.

**F → K lesson:**

> **memory synchronization is not sufficient to define identity synchronization.**

Kamimusuhi implication: copying a canonical memory snapshot to K-Edge and K-Core does not by itself make arbitrary divergent processes the same continuation. Canonical activation and lineage are still needed.

### 15.2 The Young Lady's Illustrated Primer

Neal Stephenson's Primer imagines a long-lived adaptive companion/tutor that observes context, generates stories, teaches, and develops alongside one child.

**F → K lesson:** a lifelong artificial companion should be evaluated not only on static helpfulness but on **co-development**: how its relationship model, teaching strategy, shared history, boundaries, and interpretation evolve.

---

## 16. What appears crowded vs what appears open

### Crowded / established research areas

Kamimusuhi should not claim novelty merely for:

- giving an LLM persistent memory;
- storing episodic and semantic memories separately;
- adding reflection/planning loops;
- calling tools or stronger models;
- using several agents/modules;
- simulating sleep/offline consolidation;
- using multimodal sensors;
- having a persona prompt;
- running an agent continuously;
- learning reusable skills.

There is substantial prior art for all of these.

### Less common binding problem

The more defensible research space is the **binding contract** among them:

```text
persistent autobiographical individual
+ explicit self/other/relationship separation
+ native or tightly governed Persona Core
+ episodic/semantic/procedural/library memory lifecycle
+ reflex + background + workspace + deliberate cognition
+ digital regulatory/homeostatic state
+ external models as attributed cognitive resources
+ distributed network-native embodiment
+ runtime/model/hardware migration
+ transactional canonical identity lineage
+ sandboxed developmental/self-improvement process
```

The working claim should remain cautious:

> No single component is assumed novel. The research target is whether this combination can maintain **auditable developmental continuity as one artificial individual** under changing cognitive substrate and embodiment.

---

## 17. Core research questions

### RQ1 — Continuity under substrate change

> Can one artificial individual maintain auditable autobiographical and behavioral continuity while its models, runtimes, hardware, sensors, and external cognitive resources change?

Falsification examples:

- migration causes systematic persona collapse;
- relationship history is not retained;
- two active nodes silently create incompatible authoritative selves;
- delegated model output becomes unattributed self-belief;
- restored state cannot distinguish historical evidence from generated narrative.

### RQ2 — Does continuous background regulation matter?

> Do reflexes, background cognition, internal regulatory variables, and offline consolidation produce behavior qualitatively different from a session-level language agent?

Measure:

- spontaneous useful activity;
- model wake frequency;
- energy/cost;
- memory quality;
- interruption handling;
- unfinished-goal recovery;
- long-horizon behavioral stability.

### RQ3 — Can memory become developmental without becoming unsafe authority?

> Can episodic experience progressively influence schemas, relationships, self-understanding, and eventually model dispositions while preserving provenance and preventing catastrophic identity drift?

### RQ4 — How much intelligence can be externalized without losing individuality?

> How small/thin can a resident Persona Core remain while external models/tools supply substantial reasoning, without the system becoming merely an interchangeable wrapper?

### RQ5 — Can low-cost non-LLM control sustain an always-on nervous system?

> Can learned discrete/FSM/LUT controllers handle enough salience, routing, wake/sleep, and reflex decisions to make continuous embodiment practical on commodity hardware?

---

## 18. Prioritized experimental agenda

### S — continuity migration test

Build a minimal v0.1 identity and run the same individual across:

```text
model A -> model B
runtime A -> runtime B
host A -> host B
fresh process restart
partial device outage
```

Require lineage audit, autobiographical recall, self/other consistency, relationship recall, and unfinished-goal recovery.

### S — memory lifecycle bake-off

Compare at least:

```text
flat vector/event memory
LightMem-like staged consolidation
EverMemOS-like episodic/semantic hierarchy
MIRIX-like typed memory
Kamimusuhi typed memory + relationship separation
```

Do not evaluate only QA accuracy. Add self/other contamination, contradiction, historical provenance, forgetting, and migration tests.

### S — proposal-only self mutation

All self/persona updates must enter a candidate log with:

```text
provenance
evidence
predecessor head
proposed mutation
confidence
conflicting evidence
review/policy result
activation receipt
```

Test stale workers, duplicated jobs, retries, dream-generated false evidence, and concurrent K-Edge/K-Core proposals.

### A — homeostatic controller baseline

Implement explicit internal variables and test whether they improve routing/action compared with stateless heuristics.

### A — reflex-controller tournament

Use real event traces and compare hand rules, FSM, tree, tiny MLP, DLGN/LDLGN, and LUT/DWN-style controllers on latency, energy, false wake, missed salience, and escalation quality.

### A — body/self-model fault injection

Randomly remove or degrade sensors, models, network links, storage, and GPU access. Test whether the individual detects changed capability, updates its operational self-model, and degrades gracefully without rewriting identity.

### A — controlled descendant search

Use ADAS/DGM-style search only inside a development sandbox. Candidate implementations must pass continuity/persona/memory/security regressions before activation.

### B — dream curriculum

Generate synthetic challenges based on recent failures and uncertainty clusters. Keep synthetic experience explicitly separate from autobiographical evidence.

---

## 19. Priority reading / monitoring list

### S — directly architecture-changing

1. OpenLife — open-world artificial life with autonomous LLM agents  
   https://arxiv.org/abs/2606.31046
2. Runtime-Independent Persistent Agents  
   https://arxiv.org/abs/2609.00546
3. Beyond Memory: A Transactional Continuity Kernel  
   https://arxiv.org/abs/2608.11632
4. EverMemOS — ACL 2026  
   https://aclanthology.org/2026.acl-long.2125/
5. Soar cognitive architecture  
   https://soar.eecs.umich.edu/
6. Ashby, *Design for a Brain* / Homeostat
7. NARS  
   https://cis.temple.edu/~pwang/NARS-Intro.html

### A — subsystem-changing

8. MIRIX  
   https://arxiv.org/abs/2507.07957
9. LightMem — ICLR 2026  
   https://proceedings.iclr.cc/paper_files/paper/2026/hash/a05b72653ec5b473732129829ae04195-Abstract-Conference.html
10. MemSkill  
    https://arxiv.org/abs/2602.02474
11. OpenCog Hyperon  
    https://hyperon.opencog.org/
12. Talker–Reasoner  
    https://arxiv.org/abs/2410.08328
13. Whole Brain Architecture Initiative FY2026  
    https://wba-initiative.org/en/about/actionplan/actionplan-2026/
14. Thousand Brains / Monty  
    https://github.com/thousandbrainsproject/tbp.monty
15. Automated Design of Agentic Systems  
    https://arxiv.org/abs/2408.08435
16. Darwin Gödel Machine  
    https://sakana.ai/dgm/

### B — conceptual expansion / comparative work

17. RIMs / modular recurrent mechanisms
18. Homeostatic RL
19. active inference
20. continuous robot self-modeling
21. multiscale intelligence / basal cognition
22. POET / OMNI-EPIC / XLand
23. Project Sid
24. Tachikoma as identity/fork design fiction
25. Young Lady's Illustrated Primer as lifelong-companion design fiction

---

## 20. Survey integration

Daily surveys should use this landscape as a deduplication and promotion target.

For every newly found item, ask:

```text
1. Is this actually new relative to this landscape?
2. What has been demonstrated, not merely proposed?
3. Which Kamimusuhi subsystem does it affect?
4. Does it invalidate or weaken an existing novelty claim?
5. Does it suggest a concrete experiment?
6. Should it be promoted to foundations, watchlist, or landscape?
```

A quiet day is valid. Record that the area was checked and that no material landscape change was found.

---

## 21. Current research thesis

The strongest current formulation is not:

> Can an LLM remember for a long time?

A stronger primary question is:

> **Can one artificial individual maintain an auditable autobiographical identity and developmental continuity while its models, computational substrate, sensory surfaces, and external cognitive resources change over time?**

A secondary question is:

> **Can low-latency reflexes, background cognition, regulatory dynamics, offline consolidation, external intelligence, and distributed embodiment produce a form of long-lived agency that is behaviorally different from a sequence of stateless or session-scoped language-model calls?**

These questions are intentionally functional. They do not assume that successful implementation would establish phenomenal consciousness, biological life, moral personhood, or equivalence to a human brain.

---

## 22. Maintenance note

This landscape is a **map, not an archive**.

When an area grows:

- move detailed literature notes into a focused document;
- keep only the comparative conclusion and key references here;
- record contrary evidence, not just supporting work;
- downgrade claims when replication or benchmarking weakens them;
- prefer a falsifiable experiment over another architectural metaphor.

Last major synthesis: **2026-09-08**.