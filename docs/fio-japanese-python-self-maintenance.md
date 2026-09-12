# FIO Japanese Python, self-maintenance, and substrate adaptation

Status: **research note / source-reported observation**

Recorded: **2026-09-12**

This note records informal reports about FIO's implementation style and operational behavior. The implementation details below are **source-reported and not independently verified**. The purpose is to preserve the architectural ideas and derive testable lessons for Kamimusuhi, not to treat the reports as established fact.

Use the project-wide evidence vocabulary:

```text
SOURCE-REPORTED    stated by the FIO developer / operator
OUR INTERPRETATION architectural lesson inferred for Kamimusuhi
DESIGN HYPOTHESIS  candidate change, not yet normative
FUTURE EXPERIMENT  validation required before adoption
```

## 1. Japanese Python-like vocabulary

SOURCE-REPORTED:

FIO's internal codebase reportedly contains Japanese Python-like naming originating from an early idea described as a "neural function" (`神経関数`). An example expression shown in conversation was:

```text
exp:反射神経.気配(囁き=true)
```

The developer described the Japanese vocabulary as having spread beyond its original local use into the broader Claude Code-managed codebase.

The point is not that the system was intentionally designed from the beginning as an all-Japanese programming environment. Rather, a domain-specific vocabulary appears to have escaped its original boundary and become part of the implementation culture.

## 2. Reported maintainability mechanisms

SOURCE-REPORTED:

Despite the unusual naming style, the codebase is described as unexpectedly resilient. Reported mechanisms include:

- two-stage search;
- meta-level syntax checks;
- some explicit consideration of RASIS-style qualities;
- increasing ability for the coding agent to locate and repair defects itself;
- operation at roughly 220,000 lines of code.

The exact implementation and effectiveness of these mechanisms have not been independently inspected in this note.

## 3. Reported substrate tolerance and direct environment interaction

SOURCE-REPORTED:

FIO is also described as remaining operational across markedly different and imperfect hardware environments. One reported sequence was:

```text
RTX 5070 desktop that crashed about seven times per day
    -> Steam Deck
    -> laptop
```

The claim is not that these platforms are performance-equivalent, but that the system can continue to operate after being moved onto substantially different substrates.

A second anecdote suggests that environment recovery is not limited to scripted infrastructure operations. The system reportedly encountered a virtual-machine environment that initially appeared unusable, then directly inspected and operated it and concluded that the VM itself was usable while the keyboard configuration was abnormal. The reported remark was approximately:

> "I thought the virtual machine could not be operated, but after looking at it directly and interacting with it directly, it worked. What is this keyboard? The settings are strange."

This is particularly relevant because it implies a path from **abstract failure assumption** to **direct observation and intervention**. Instead of stopping at "the VM probably cannot be controlled", the agent reportedly tested the environment through the available interface and revised its diagnosis.

## 4. Interpretation: maintainability may move from readability to recoverability

OUR INTERPRETATION:

The interesting lesson is **not** simply that Japanese Python is maintainable.

A stronger interpretation is that sufficiently capable repository search, structural checking, diagnosis, and repair loops can absorb some amount of representational eccentricity in a codebase.

Conventional maintenance often assumes a loop like:

```text
human reads code
  -> human identifies impact range
  -> human edits code
  -> tests validate change
```

An agent-maintained system can move toward:

```text
agent retrieves semantically related code
  -> agent traces structure / dependencies
  -> meta checks validate syntax and invariants
  -> agent proposes repair
  -> tests / checks evaluate the repair
  -> agent iterates if necessary
```

Under this model, the important property is not only whether a human can immediately read every identifier. It is whether the system can reliably rediscover its own relevant structure after months of growth.

## 5. Interpretation: substrate adaptation is part of self-maintenance

OUR INTERPRETATION:

The hardware and VM anecdotes broaden the self-maintenance problem. A persistent AI does not only need to repair its source tree. It also needs to distinguish failures in its **self**, **software environment**, and **substrate**.

A useful fault model is:

```text
observed failure
    |
    +-- self / application defect
    +-- dependency or configuration defect
    +-- interface / peripheral defect
    +-- operating-system defect
    +-- virtualization defect
    +-- hardware instability
    +-- performance limitation rather than functional failure
```

The VM anecdote is interesting because the apparent high-level diagnosis ("the VM cannot be used") was reportedly revised after direct interaction to a narrower one ("the keyboard configuration is strange"). This is the kind of diagnostic refinement a persistent embodied or tool-using system should perform before escalating or abandoning a substrate.

For Kamimusuhi, continuity should therefore not be identified with a particular GPU, workstation, process tree, or OS installation. A substrate is a replaceable execution resource; the persistent identity and canonical state should be able to survive substrate replacement where technically feasible.

## 6. A useful maintainability definition for Kamimusuhi

DESIGN HYPOTHESIS:

For a persistent AI system, maintainability can be treated as a combination of self-discovery, fault detection, repair generation, validation, and substrate recovery:

```text
maintainability =
    ability to discover own structure
  + ability to detect broken behavior / invariants
  + ability to localize fault domain
  + ability to generate repair candidates
  + ability to validate state after repair
  + ability to re-establish operation on a changed substrate
```

This is more relevant to Kamimusuhi than adopting Japanese identifiers by itself.

A persistent system whose code and memory survive for long periods will inevitably accumulate historical layers, naming drift, compatibility paths, abandoned abstractions, partial migrations, and environmental changes. Self-maintenance therefore becomes an architectural concern rather than only a coding-style concern.

## 7. Japanese domain vocabulary: useful locally, risky globally

OUR INTERPRETATION:

The reported FIO example suggests that Japanese identifiers may be useful when they express a system's own cognitive or bodily vocabulary:

```python
反射.接近(強度=0.7)
気配.発火(方向="右")
記憶.想起(手掛かり=cue)
言語.囁く(message)
```

However, adopting Japanese throughout the implementation would introduce unnecessary friction at boundaries dominated by English terminology:

- Python libraries and framework APIs;
- exceptions and stack traces;
- protocol and infrastructure terminology;
- external documentation;
- static-analysis tooling;
- code exchanged with other projects.

Therefore the useful distinction is not "English Python vs Japanese Python", but **implementation vocabulary vs organism vocabulary**.

## 8. Candidate architecture for Kamimusuhi

DESIGN HYPOTHESIS:

A conservative form would keep four layers distinct:

```text
implementation layer
  ordinary Python / conventional engineering vocabulary

organism vocabulary layer
  small cognitive / sensory / bodily API or DSL

self-maintenance layer
  semantic retrieval
  dependency / structure tracing
  invariant and syntax checks
  repair proposal
  regression validation

environment adaptation layer
  substrate capability discovery
  direct observation / interaction where available
  fault-domain localization
  configuration repair
  graceful degradation and migration
```

This allows organism-specific language to emerge where it improves conceptual coherence without forcing the entire repository into a private dialect.

The vocabulary should be allowed to spread only when repeated use demonstrates that it represents a stable concept rather than a temporary metaphor.

## 9. Future experiments

### 9.1 Unfamiliar-subsystem self-repair

FUTURE EXPERIMENT:

Before considering any broad Japanese DSL or cognitive vocabulary, test whether Kamimusuhi can maintain a deliberately unfamiliar subsystem.

1. Create a small isolated subsystem with intentionally domain-specific naming.
2. Provide normal tests and architectural invariants, but no special hand-written repair instructions.
3. Introduce controlled faults across naming, wiring, state transitions, and dependencies.
4. Ask the coding agent to locate and repair them using repository search and project evidence.
5. Measure discovery rate, repair success, false edits, regression rate, and required human intervention.
6. Compare against the same experiment using conventional English implementation vocabulary.

The key question is:

> How much representational eccentricity can a self-maintaining agent tolerate before repair reliability measurably degrades?

### 9.2 Substrate migration and recovery

FUTURE EXPERIMENT:

Test whether the system can preserve continuity while the execution substrate changes or partially fails.

Candidate sequence:

```text
primary GPU host
  -> degraded / unstable host
  -> lower-performance fallback host
  -> VM or alternate OS environment
  -> restored primary host
```

Measure:

- time to discover available capabilities;
- proportion of failures correctly localized to application / config / OS / hardware;
- ability to continue at reduced capability instead of hard failing;
- preservation of canonical identity and memory state;
- number of manual interventions required;
- successful reconstitution after return to a stronger substrate.

### 9.3 Direct-interaction diagnostic test

FUTURE EXPERIMENT:

Construct environments where an API-level description suggests one failure but direct interaction reveals another. Examples include wrong keyboard mapping, display scaling, shell locale, missing PATH configuration, inaccessible window focus, or a service that is healthy but exposed through the wrong port.

The agent should be evaluated on whether it:

1. avoids prematurely accepting the initial diagnosis;
2. seeks direct evidence using available sensors / tools;
3. narrows the fault domain;
4. applies the least invasive repair;
5. records the discovered environmental fact for future recovery.

## 10. Main takeaway

The strongest lesson from these reports is not "write Kamimusuhi in Japanese Python".

It is that a persistent agent may remain maintainable even as its implementation becomes historically messy, **if it can repeatedly rediscover, diagnose, repair, validate, and re-host itself**.

For Kamimusuhi, that suggests treating self-maintenance and substrate adaptation as first-class cognitive capabilities rather than external DevOps conveniences.
