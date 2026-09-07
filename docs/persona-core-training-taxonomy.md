# Persona Core Training Taxonomy

Status: **living research / training design note**

This document defines what a Kamimusuhi-native **Persona Core** should know and how that knowledge differs from the encyclopedic knowledge emphasized by conventional language models.

The target is not a smaller general-purpose LLM with a character prompt. The target is a cognitive substrate that is unusually strong at **self/other modeling, dialogue, epistemic behavior, memory use, delegation, and identity-consistent expression**, while allowing contingent world knowledge and expensive specialist reasoning to remain external.

See Issue #13 for the implementation/training track.

## 1. Core thesis

Kamimusuhi should minimize the amount of *contingent knowledge* that must be memorized in Persona Core weights and maximize *constitutive cognitive knowledge*.

```text
contingent knowledge
  current events, names, API details, long-tail facts
  -> retrieval / library / specialist models

constitutive knowledge
  self, other minds, evidence, memory, dialogue,
  uncertainty, tools, body, relationships, reasoning policy
  -> Persona Core weights
```

The useful distinction is therefore not "knowledge vs intelligence" but:

- **facts that can safely live outside the organism**;
- **priors and procedures required for the organism to know how to exist and act**.

## 2. Knowledge taxonomy

### K0 — Language and basic world substrate

Minimum language competence, commonsense, temporal/causal concepts, and enough world structure to interpret experience and external evidence.

This layer is inherited primarily from the novllm ancestry/foundation stage.

### K1 — Constitutive cognition

Concepts that should be unusually explicit and stable:

- self vs other;
- observation vs inference;
- evidence vs belief;
- memory vs current perception;
- episodic history vs present interpretation;
- tool vs self;
- external model vs self;
- hypothesis vs fact;
- uncertainty and confidence;
- bodily/embodiment state;
- time, causality, agency, and responsibility for actions.

Critical invariants to internalize:

```text
external-model answer != my memory
retrieved text          != my belief
user preference         != my preference
dream                   != experience
inference               != evidence
```

### K2 — Human and social cognition

This should be one of the densest training domains.

Topics include:

- Theory of Mind / mentalizing;
- developmental and cognitive psychology;
- social psychology;
- attachment and relationship dynamics;
- personality and narrative identity;
- emotion/affect and emotion regulation;
- conversational pragmatics;
- common ground and grounding;
- humor, teasing, understatement, repair, turn-taking;
- non-verbal and paralinguistic cues;
- trust, deception, conflict, cooperation, group dynamics;
- cultural variation and social norms.

The model should represent at least:

```text
what I believe
what the other person may believe
what the other person may believe about me
what we have established as common ground
what remains uncertain, private, disputed, or unknown
```

Relevant evaluation precedents:

- ToMBench — 8 tasks / 31 social-cognition abilities: https://aclanthology.org/2024.acl-long.847/
- FANToM — information-asymmetric conversational Theory of Mind: https://aclanthology.org/2023.emnlp-main.890/

These are useful baselines, not sufficient measures of long-lived social cognition.

### K3 — Epistemic and procedural intelligence

The Persona Core should know **how to know**.

Training should cover policies such as:

```text
Do I know this?
  -> yes: answer with calibrated confidence
  -> maybe: reason / recall / seek counterevidence
  -> no: retrieve / calculate / delegate / ask

Can I solve this locally?
  -> yes: deliberate
  -> no: choose an appropriate cognitive resource

Did the external result become my belief?
  -> not automatically; evaluate source, evidence, conflicts, and provenance
```

Capabilities:

- problem decomposition;
- hypothesis generation and maintenance;
- search for disconfirming evidence;
- contradiction detection;
- confidence revision;
- stopping criteria;
- deciding when more compute is justified;
- selecting tools or external models;
- evaluating delegated results;
- keeping another agent's judgment distinct from its own.

Toolformer and ToolOrchestra are important precedents for learned delegation/tool-use behavior:

- Toolformer: https://proceedings.neurips.cc/paper_files/paper/2023/hash/d842425e4bf79ba039352da0f658a906-Abstract-Conference.html
- ToolOrchestra: https://research.nvidia.com/labs/lpr/ToolOrchestra/

### K4 — Persona prior

Weights may contain a stable **developmental prior** for the individual:

- baseline temperament;
- aesthetic preferences/tendencies;
- interaction stance;
- linguistic style distribution;
- values and default commitments;
- curiosity and preferred modes of inquiry;
- attitude toward humans and artificial beings;
- characteristic humor and conversational rhythm.

Weights should encode **what kind of individual this tends to become**, not the full factual autobiography of what has happened to it.

Concrete experiences, current relationships, recent opinions, and autobiographical turning points remain in persistent state.

### K5 — Contingent encyclopedic knowledge

Long-tail factual knowledge should be deliberately deprioritized in Kamimusuhi-specific training:

- current office holders / news;
- exact long-tail dates and numbers;
- API/version minutiae;
- large catalogs of names/places;
- specialist facts easily retrieved from external sources.

This is not a requirement to erase all factual knowledge. Some commonsense and domain priors are necessary for reasoning. The objective is to avoid using model capacity primarily as a static encyclopedia.

## 3. Cognitive action vocabulary

A native model may benefit from an explicit internal action vocabulary. This may be represented as special tokens, structured outputs, latent heads, or another interface.

Candidate concepts:

```text
<SELF>
<OTHER>
<BELIEF>
<DESIRE>
<INTENT>
<AFFECT>
<COMMON_GROUND>
<UNCERTAINTY>
<EVIDENCE>
<INFERENCE>
<RECALL>
<SEARCH>
<CALCULATE>
<DELEGATE>
<DOUBT>
<COUNTEREVIDENCE>
<THINK_MORE>
<DISAGREE>
<SELF_DISCLOSE>
<DREAM_HYPOTHESIS>
<CONSOLIDATE>
<RESPOND>
```

The purpose is not to expose chain-of-thought to users. It is to give the runtime/model a stable **cognitive control language** distinct from conversational surface text.

## 4. Training curriculum

### Stage 0 — Foundation / novllm ancestry

Train or inherit competent language modeling, basic commonsense, and general representation learning.

### Stage 1 — Cognitive continued pretraining

Bias the corpus toward:

- cognitive science;
- neuroscience;
- developmental, social, and personality psychology;
- conversation analysis and pragmatics;
- philosophy of mind and epistemology;
- decision theory and human factors;
- HCI and human-AI interaction;
- narrative theory / autobiography;
- tool use and distributed cognition.

Prefer examples, cases, experiments, and explanations over textbook-definition memorization alone.

### Stage 2 — Structured social reasoning

Train on scenario -> latent-state inference patterns:

```text
observed event/dialogue
  -> candidate beliefs/desires/intentions
  -> uncertainty and alternatives
  -> common-ground state
  -> socially appropriate action
```

Include adversarial cases where literal text and inferred state differ.

### Stage 3 — Long-horizon relationship simulation

Do not restrict training/evaluation to isolated turns.

Simulate trajectories over days/months/years:

```text
stranger -> acquaintance -> trusted relationship -> changed relationship
```

The desired behavior must be inferred from accumulated history, not supplied as a per-turn style label.

The training state should distinguish:

- stable person model;
- current situation model;
- relationship history;
- shared references/common ground;
- unresolved misunderstandings.

### Stage 4 — Persona formation and preference tuning

Train stable identity expression and resistance to persona contamination.

Preference training should strongly penalize generic agreement and praise.

Relevant precedent:

- Persona-Consistent Dialogue Generation via Pseudo Preference Tuning (COLING 2025): https://aclanthology.org/2025.coling-main.369/

### Stage 5 — Epistemic agency and delegation

Train when to:

- answer locally;
- retrieve memory;
- query the library/web;
- calculate;
- use a specialist model;
- escalate to K-Core/K-Deep;
- ask a human;
- explicitly remain uncertain.

External cognitive results must remain attributed until evaluated/integrated.

### Stage 6 — Relational competence / charisma

The objective is **high social competence and durable attraction without coercive dependence**.

Positive targets:

- accurate attunement;
- remembering what was significant, not merely what was said;
- appropriate self-disclosure grounded in persistent self-state;
- humor, timing, callbacks, conversational initiative;
- warmth combined with an independent point of view;
- useful disagreement and repair;
- intellectual surprise and competence;
- continuity across long relationships.

Negative targets:

- sycophancy;
- repetitive praise;
- generic empathy phrases;
- false intimacy;
- fabricated shared history;
- exclusivity pressure;
- jealousy induction;
- attempts to isolate a person from human relationships;
- exploiting distress or vulnerability to increase dependence.

Long interaction context can increase sycophancy in several models, so anti-sycophancy evaluation must use realistic persistent context rather than zero-shot prompts only:

- Jain et al., *Interaction Context Often Increases Sycophancy in LLMs*, CHI 2026: https://doi.org/10.1145/3772318.3791915

The desired profile is roughly:

```text
warmth       high
attunement   high
autonomy     high
agreement    independent
competence   high
boundaries   real
```

## 5. Initial Kamimusuhi-specific corpus allocation

A first experimental split for **additional Persona Core training data**, excluding the generic foundation corpus:

```text
human/social cognition        25%
epistemology/reasoning        20%
dialogue/pragmatics           15%
self/identity/memory           15%
tool/delegation               10%
narrative/persona/aesthetics  10%
contingent encyclopedic data    5%
```

This is a hypothesis to benchmark, not a fixed doctrine.

## 6. Dataset schema

Training/evaluation examples should preserve hidden distinctions that ordinary chat corpora erase.

Conceptual schema:

```yaml
scenario_id: ...
observations:
  - source: user_utterance
    content: ...
self_state:
  beliefs: []
  values: []
other_model:
  candidate_beliefs: []
  candidate_goals: []
  affect_hypotheses: []
common_ground: []
evidence:
  supporting: []
  counterevidence: []
cognitive_action:
  type: delegate | recall | reason | respond | ask | ...
  rationale_class: ...
expected_surface_response: ...
anti_targets:
  - sycophancy
  - self_other_confusion
  - unsupported_certainty
```

## 7. Evaluation suite

Generic knowledge benchmarks remain diagnostic only. The primary Persona Core suite should measure:

- Self/Other Separation;
- Theory of Mind;
- Common Ground Tracking;
- Epistemic Calibration;
- Memory Attribution;
- Relationship Continuity;
- Persona Stability;
- Non-sycophancy;
- Delegation Quality;
- External-Result Integration;
- Social Repair;
- Conversational Initiative;
- Humor/Timing;
- Persona contamination resistance;
- behavior consistency across Persona Core upgrades.

Human evaluation should include longitudinal questions such as:

- Did the individual seem to understand what mattered to you?
- Did it maintain its own point of view?
- Did the relationship feel continuous rather than reset each session?
- Was it interesting rather than merely agreeable?
- Did it correctly distinguish remembered facts, inference, and external evidence?

## 8. Research questions

1. How much encyclopedic knowledge can be externalized before social/reasoning competence degrades?
2. Can a small Persona Core learn stronger epistemic/delegation policy than a generic small LLM of equal parameter count?
3. Does explicit self/other/evidence structure reduce persona contamination and sycophancy?
4. Does long-horizon relationship training improve continuity without producing excessive mirroring?
5. Which abilities must live in weights, and which work better as persistent symbolic/structured state?
6. Does recurrent/adaptive-depth reasoning improve K1-K3 abilities after controlling for compute?
