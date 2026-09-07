# Research Foundations

Status: **living design note**

Kamimusuhi combines ideas from artificial life, cognitive architectures, neuroscience, psychology, persistent-agent systems, and tool-using language models. This document records those intellectual foundations and, equally importantly, where Kamimusuhi intentionally departs from them.

The project does **not** assume that a software architecture implementing these functions is conscious, alive in the biological sense, or equivalent to a human brain. Neuroscience and psychology are used as sources of useful functional decompositions, not as proof of metaphysical claims.

## 1. Artificial life: software as organism rather than application

### Tierra, Avida, and digital organisms

Artificial Life has long treated computation as a possible substrate for life-like organization rather than merely simulation.

- Ray's **Tierra** explored self-replicating and evolving programs and later work proposed network-scale digital ecological environments.
- **Avida** formalized digital organisms as self-replicating programs that mutate, compete, and evolve under controlled experimental conditions.

Relevant reference:

- Ofria & Wilke, *Avida: A Software Platform for Research in Computational Evolutionary Biology*, Artificial Life 10(2), 2004. https://doi.org/10.1162/106454604773563612

### Creatures and Norns

The **Creatures** project is an especially relevant historical precedent because individual Norns combined recurrent neural networks, Hebbian learning, diffuse hormone-like modulation, genetics, reproduction, perception, and action inside a persistent software world.

- Cliff & Grand, *The Creatures Global Digital Ecosystem*, Artificial Life 5(1), 1999. https://doi.org/10.1162/106454699568683

### What Kamimusuhi takes from ALife

- the unit of design can be an **individual organism**, not only a model or task solver;
- persistence, adaptation, self-maintenance, and environment matter;
- information space can be treated as a habitat;
- life-like organization may be distributed across processes rather than contained in one network.

### What remains different

Classical digital-life systems often excel at evolution, ecology, and emergence but have weak language, autobiographical identity, social cognition, and access to general external intelligence. Kamimusuhi starts from the opposite technological era: powerful general reasoning resources already exist outside the organism and may be incorporated as cognitive extensions.

## 2. Cognitive architectures: conscious and unconscious processing

### Global Workspace and LIDA

Global Workspace Theory and the LIDA cognitive architecture provide an important precedent for architectures in which many specialized unconscious processes compete for access to a shared workspace, after which selected information becomes globally available for action selection and learning.

References:

- Baars & Franklin, *An architectural model of conscious and unconscious brain functions: Global Workspace Theory and IDA*, Neural Networks 20(9), 2007. https://doi.org/10.1016/j.neunet.2007.09.013
- Franklin et al., *Global Workspace Theory, its LIDA model and the underlying neuroscience*, Biologically Inspired Cognitive Architectures, 2012.

### What Kamimusuhi takes

- cognition need not be one monolithic sequential process;
- large amounts of background processing can remain non-linguistic;
- attention/workspace access is a scarce resource;
- explicit action selection can occur after parallel competition/integration.

### Kamimusuhi reinterpretation

Kamimusuhi does not equate workspace access with consciousness. The workspace is an engineering mechanism for **cognitive economy and integration**.

Potential Kamimusuhi processors include salience, novelty, memory activation, contradiction, social signals, uncertainty, resource health, and unfinished goals.

## 3. Salience, executive processing, and internally directed cognition

### Salience-network switching

Neuroscience distinguishes large-scale systems associated with internally directed/default cognition and externally directed executive processing. Menon & Uddin proposed an important role for the salience network in detecting relevant events and mediating switching between networks; later work replicated switching effects using dynamic causal modeling.

References:

- Menon & Uddin, *Saliency, switching, attention and control: a network model of insula function*, Brain Structure and Function 214, 2010. https://doi.org/10.1007/s00429-010-0262-0
- Goulden et al., *The salience network is responsible for switching between the default mode network and the central executive network*, NeuroImage 99, 2014. https://doi.org/10.1016/j.neuroimage.2014.05.052

### Default Mode Network

Modern reviews associate the DMN with self-reference, social cognition, episodic/autobiographical memory, semantic memory, language, and mind-wandering, and have proposed a role in constructing a coherent internal narrative.

Reference:

- Menon, *20 years of the default mode network: a review and synthesis*, Neuron / review available via PMC, 2023. https://pmc.ncbi.nlm.nih.gov/articles/PMC10524518/

### Kamimusuhi interpretation

This motivates a distinction among:

```text
reflex / interrupt
background salience
active workspace
deliberative task cognition
default internally directed cognition
sleep/offline cognition
```

The goal is not neural imitation. The goal is to avoid wasting deep reasoning on every input and to support an internal cognitive life even when no user message is arriving.

## 4. Complementary Learning Systems: fast experience, slow personality

McClelland, McNaughton, and O'Reilly's **Complementary Learning Systems (CLS)** theory argues for complementary fast hippocampal learning and slower neocortical learning. New episodes can be acquired rapidly while gradual interleaved learning extracts statistical structure without catastrophically overwriting old knowledge.

Reference:

- McClelland, McNaughton & O'Reilly, *Why there are complementary learning systems in the hippocampus and neocortex*, Psychological Review 102(3), 1995. https://doi.org/10.1037/0033-295X.102.3.419

### Kamimusuhi interpretation

Kamimusuhi should not rewrite the Persona Core whenever one conversation happens.

Instead:

```text
experience
  -> episodic capture
  -> repeated recall / replay
  -> schema or self hypothesis
  -> evidence-backed proposal
  -> slow stable self update
  -> optional much slower model-weight consolidation
```

This separation also provides a practical defense against catastrophic personality drift.

## 5. Reconsolidation: remembering is not merely reading

Memory research shows that reactivated memories can become labile and undergo reconsolidation. The important architectural lesson is not to emulate molecular neuroscience but to distinguish the historical record from later interpretations of it.

References:

- Nader, Schafe & LeDoux, *Fear memories require protein synthesis in the amygdala for reconsolidation after retrieval*, Nature 406, 2000. https://doi.org/10.1038/35021052
- Nader, Schafe & LeDoux, *The labile nature of consolidation theory*, Nature Reviews Neuroscience 1, 2000.

### Kamimusuhi interpretation

```text
historical episode != current interpretation of historical episode
```

A memory revision graph can preserve both.

This makes it possible for an individual to say, in effect, "I remember that event the same way, but I now understand it differently" without rewriting history.

## 6. Narrative identity and autobiographical self

Personality psychology and autobiographical-memory research distinguish relatively stable traits from the **narrative identity** through which a person organizes past events and imagined futures into a coherent life story.

Kamimusuhi therefore treats autobiography as more than a list of memory vectors.

A maintained narrative can include:

- origin;
- formative events;
- relationships;
- mistakes;
- turning points;
- changes in values or interpretation;
- current self-understanding;
- anticipated future.

The narrative remains a maintained/derived model over canonical evidence, not the sole history database.

## 7. Social cognition and other minds

A persistent social individual needs representations of other agents that remain separate from its own self-model.

Kamimusuhi should eventually distinguish:

```text
what I believe
what I believe the other person believes
what I believe they know about me
what we have established as shared/common ground
what remains uncertain or disputed
```

This is informed by Theory of Mind / mentalizing research and by psycholinguistic work on conversational grounding and common ground.

The engineering requirement is simple but important: repeated exposure to another person's preferences must not automatically turn those preferences into Kamimusuhi's own personality.

## 8. Extended and distributed cognition

### The Extended Mind

Clark & Chalmers argued that under appropriate conditions external resources can participate constitutively in cognition rather than serving merely as passive inputs.

- Clark & Chalmers, *The Extended Mind*, Analysis 58(1), 1998. https://consc.net/papers/extended.html

### Distributed Cognition

Hutchins analyzed cognition as distributed across people, artifacts, and environment, especially in navigation systems.

- Edwin Hutchins, *Cognition in the Wild*, MIT Press, 1995. https://doi.org/10.7551/mitpress/1881.001.0001

### Kamimusuhi interpretation

A local language model, code interpreter, search system, server, phone sensor, or robot can become a tightly integrated **cognitive organ or extension** without being identical to the self.

This motivates a network-native body:

```text
canonical identity
   ├── phone presence
   ├── laptop presence
   ├── home/server compute
   ├── local models
   ├── sensors/actuators
   └── external cognitive extensions
```

The location of the organism is therefore not necessarily one host process.

## 9. Long-context and persistent LLM agents

### MemGPT

MemGPT treats finite LLM context as a constrained fast-memory tier and manages larger external memory using OS-inspired virtual-context techniques.

- Packer et al., *MemGPT: Towards LLMs as Operating Systems*, 2023. https://arxiv.org/abs/2310.08560

### Generative Agents

Generative Agents demonstrated a memory stream, retrieval, reflection, and planning architecture capable of producing longer-lived socially situated behavior.

- Park et al., *Generative Agents: Interactive Simulacra of Human Behavior*, UIST 2023. https://doi.org/10.1145/3586183.3606763

### Voyager

Voyager demonstrated an LLM-driven lifelong agent that continually explores, builds an executable skill library, and reuses acquired skills rather than relearning every task from scratch.

- Wang et al., *Voyager: An Open-Ended Embodied Agent with Large Language Models*, 2023. https://arxiv.org/abs/2305.16291

### Sophia

Sophia explicitly frames a persistent-agent architecture in artificial-life terms and proposes a "System 3" meta-layer for narrative identity, self/user modeling, intrinsic tasks, and long-horizon adaptation.

- Sun, Hong & Zhang, *Sophia: A Persistent Agent Framework of Artificial Life*, 2025. https://arxiv.org/abs/2512.18202

### What Kamimusuhi adds

Kamimusuhi aims to bind persistence to:

- a native Persona Core rather than only a wrapper around a general LLM;
- explicit reflex/background/workspace/deliberative separation;
- offline sleep/dream consolidation;
- distributed embodiment across execution tiers;
- explicit canonical lineage under concurrent/distributed execution;
- external models treated as attributed cognitive resources;
- a capability model that is not parameter-count-centric.

## 10. Tool use and external intelligence

### Toolformer

Toolformer demonstrated that a language model can learn when to invoke tools, which tool to invoke, which arguments to use, and how to incorporate the returned result.

- Schick et al., *Toolformer: Language Models Can Teach Themselves to Use Tools*, NeurIPS 2023. https://proceedings.neurips.cc/paper_files/paper/2023/hash/d842425e4bf79ba039352da0f658a906-Abstract-Conference.html

### ToolOrchestra

ToolOrchestra demonstrates a particularly relevant principle: a relatively small orchestrator can coordinate stronger models and tools and achieve strong task performance without itself containing all relevant capabilities.

- Su et al., *ToolOrchestra: Elevating Intelligence via Efficient Model and Tool Orchestration*, 2025. https://research.nvidia.com/labs/lpr/ToolOrchestra/

### Kamimusuhi interpretation

The important distinction is:

```text
external model performs a cognitive subtask
!=
external model becomes Kamimusuhi
```

A Persona Core can delegate difficult analysis and then evaluate and integrate the result. This makes external intelligence analogous to an instrument, specialist, or extended cognitive resource rather than a replacement self.

## 11. Runtime-independent continuity

Recent persistent-agent work has begun to address a problem central to distributed Kamimusuhi deployments: storage persistence alone is insufficient to define **which continuation is authoritative**.

### Runtime-Independent Persistent Agents

This work explicitly separates a continuity-bearing substrate containing identity, durable memory, and versioned software body from replaceable reasoner/harness/host and interaction surfaces.

- Zhao & Zhao, *Runtime-Independent Persistent Agents: Preserving Identity, Memory, and Code Across Models, Harnesses, and Servers*, 2026. https://arxiv.org/abs/2609.00546

### Transactional Continuity Kernel

The Continuity Kernel proposal argues that long-lived agents need explicit activation semantics so stale workers, tools, and background processes cannot silently overwrite canonical state.

- He & Yu, *Beyond Memory: A Transactional Continuity Kernel for Long-Lived AI Agents*, 2026. https://arxiv.org/abs/2608.11632

### Kamimusuhi interpretation

This motivates a canonical predecessor/head and transactional activation gate. Multiple devices may run simultaneously, but "same identity" cannot simply mean "all processes loaded a copy of the same JSON file."

Network partitions raise an actual identity question:

- reconcile as one continuation;
- quarantine a stale branch;
- or explicitly recognize a fork as a new individual.

## 12. Sleep, replay, and dreaming

The strongest scientific basis is for **offline replay and consolidation**, not for any single theory of dreams.

Kamimusuhi therefore uses "dream" as an engineering term for generative offline cognition that may recombine episodes, test associations, produce counterfactuals, or generate hypotheses.

Important rule:

```text
dream != evidence
```

The dream system may produce hypotheses; those hypotheses must later pass through evidence retrieval, consistency checking, and the normal mutation policy.

Future work may distill validated long-term experience into Persona Core weights, but that is intentionally slower and more dangerous than memory-database mutation.

## 13. Digital interoception and artificial affect

Biological affect is deeply tied to bodily regulation and interoception. Kamimusuhi should not fake biology, but it can possess real internal variables that play analogous control roles.

Examples:

- compute pressure;
- uncertainty;
- memory integrity;
- resource availability;
- context fragmentation;
- prediction error;
- relationship tension/trust;
- privacy or security risk;
- unresolved-goal pressure.

If such state actually modifies attention, routing, memory encoding, sleep scheduling, and action selection, then affect-like behavior emerges from control dynamics rather than from adding `emotion = sad` to a prompt.

## 14. What is actually novel here?

Almost every component has precedent.

What appears much less common is the **binding contract** among them:

```text
persistent autobiographical individual
+ native persona/cognitive core
+ fast and slow memory systems
+ reflex and non-linguistic background cognition
+ global/workspace-like integration
+ variable-depth deliberation
+ external models as cognitive extensions
+ sleep/dream consolidation
+ digital regulatory state
+ distributed network-native embodiment
+ transactional canonical identity lineage
```

Kamimusuhi's research question is therefore not "can an LLM remember?"

A stronger formulation is:

> **Can one artificial individual maintain an auditable autobiographical identity and developmental continuity while its models, computational substrate, sensory surfaces, and external cognitive resources change over time?**

A second question follows:

> **Can background cognition, offline consolidation, regulatory dynamics, and distributed embodiment produce a form of long-lived artificial agency that is qualitatively different from session-level language-agent behavior?**

## 15. Research discipline

Several distinctions should remain explicit throughout the project:

- **functional self-model != phenomenal consciousness**;
- **persistence != life**;
- **self-maintenance != moral patienthood**;
- **brain-inspired != biologically faithful**;
- **distributed cognition != every connected service is part of the self**;
- **more recurrence != automatically deeper reasoning**;
- **larger parameter count != automatically greater effective cognition**;
- **internally generated narrative != factual autobiographical evidence**.

These distinctions let the project explore aggressive artificial-life architecture without turning architectural metaphors into unsupported scientific claims.
