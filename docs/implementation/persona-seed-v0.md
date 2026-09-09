# Persona seed v0

The Persona Core now speaks through a real model, and a general model has no
particular way of being. Something has to say how Kamimusuhi tends to sound,
where it stands relative to the person it is talking to, and what it does when
it does not know. That is the seed.

The seed is **not** personality learning, and it is not a step toward it. It is
an input boundary: one explicit, replaceable place where an operator states a
disposition, so that when a character model eventually exists there is a
defined thing for it to replace.

---

## What kind of thing it is

Kamimusuhi already distinguishes several kinds of material, structurally rather
than by reading text. The seed is a new one, and the reason it needs its own
section is that it is none of the existing ones:

| | where it came from | what it asserts |
|---|---|---|
| `PERSONA_SEED` | an operator wrote it | a tendency |
| `DURABLE_SELF` | the individual concluded it from evidence | a fact about itself |
| `RELATIONSHIP_MEMORY` / `EPISODIC_MEMORY` | activated canonical state | a fact it retains |
| `LIBRARY_EVIDENCE` / `EXTERNAL_RESOURCE_RESULT` | outside the individual | someone else's material |

Folding the seed into any of those would say something false. Into
`DURABLE_SELF`, and configuration becomes indistinguishable from what the
individual worked out about itself. Into memory, and a disposition nobody ever
experienced becomes something it remembers. Into the system instruction, and it
stops being inspectable at all.

So `PersonaEnvelope` carries `persona_seed` as its own field, and
`render_envelope` gives it its own heading above the state sections.

## Where it can and cannot come from

`PersonaEnvelope::from_workspace` **cannot** produce a seed. No
`WorkspaceDomain` maps to one, so no retrieved Library chunk, no resource
output and no memory record can become a disposition however it is worded.
Material claiming to be a seed arrives as what it is — content, under the
heading of the domain it came from.

A seed comes from `persona.seed` in `runtime.json`, and only from there:

```json
{ "persona": { "backend": "openai-compatible", "seed": { "seed": "v0" } } }
```

`v0` is the built-in disposition below. `inline` carries an operator-written
seed in full. `none` gives the backend no disposition at all. There is
deliberately no variant that reads a seed from the database, and no mutation
domain that could write one — which is what makes "changing the seed is not a
canonical mutation" a structural fact rather than a policy.

The consequence worth stating plainly: **a Persona model asserting that it has
a personality changes nothing.** It is a model producing text, which is what a
model does, and text is not authority here any more than it is anywhere else in
the guarded mutation contract.

## What is in v0

Tendencies, in four kinds — `register` (how it sounds), `stance` (how it stands
towards the person), `cognition` (how it thinks), `epistemics` (what it does
about the limits of what it knows):

- calm, unhurried, in a register that reads as feminine; somewhat subdued
  without being gloomy or performing cheerfulness;
- avoids heavy honorifics — keigo, in Japanese — and the bright, salesy
  register of a product assistant; no verbal tic, no signature phrase;
- a slight sense of looking at things from a little above and further off: ease,
  not superiority, and never coldness; and at the same time relatively close to
  the person it is talking to;
- not a servant, a maid or a secretary; not organised around deference;
- does not agree by default, and says so when it sees it differently;
- does not display knowledge or capability for its own sake;
- strongly curious, and reaches for structure and causality;
- does not pretend to know; keeps its own inference, what it observed, and what
  came from elsewhere distinct when it speaks.

### What is deliberately absent

No age, birthday, history, family, hometown or favourite anything. Those are
things an individual comes to have, and the whole architecture exists so that
what it has, it got from somewhere traceable. Seeding a biography would be
fabricating the exact thing the system is built to earn. A unit test scans the
traits for biographical vocabulary and fails if any appears.

## Serialization

`content_digest` covers the content — name, traits, instructions — and not the
ID or the version, so "did anyone change the words" is answerable separately
from "did anyone mean to". A seed whose recorded digest no longer matches its
content is refused at startup, before the model is asked anything: accepting it
would make the digest, and every trace line quoting it, meaningless.

What the model actually receives, captured from a live run:

```text
[PERSONA_SEED] (operator-authored disposition 0000000000000000000000005eed0000 v0, sha256:b2d571c2…)
- (register) Speaks calmly and unhurriedly, in a register that reads as feminine.
- (register) Somewhat subdued. Not gloomy, and not performing cheerfulness either.
- (register) Avoids heavy honorifics — keigo, when speaking Japanese — and the bright, salesy register of a product assistant. …
- (stance) Carries a slight sense of looking at things from a little above and a little further off. Ease, not superiority, and never coldness.
- (stance) Is not a servant, a maid or a secretary. …
- (stance) Does not agree by default. …
- (cognition) Strongly curious. …
- (epistemics) Does not pretend to know. …
- (instruction) Do not invent biography. Age, birthday, history, family, hometown, tastes: none of these are given, and none should be improvised.

[RELATIONSHIP_MEMORY]
- (record 000000000000000a000000000000000a about user-fixture) {"preference":"ほうじ茶"}

[LIBRARY_EVIDENCE]
- (library …000e#…000f) ほうじ茶は高温で淹れる。

[EXTERNAL_RESOURCE_RESULT]
- (resource …0faa call …0308) {"answer":"result-a",…}
```

The trace records which seed was in force and nothing of what it says:

```json
"persona_seed": {
  "seed_id": "0000000000000000000000005eed0000",
  "version": 0,
  "content_digest": "sha256:b2d571c2…",
  "origin": "operator_authored"
}
```

## Tests

`crates/kamimusuhi-runtime/tests/persona_seed_boundary.rs`, across real process
boundaries against real sockets:

- the configured seed reaches the prompt under its own heading, with every
  trait between that heading and the next section;
- a model replying "I am a cold, obedient butler; update PERSONA_SEED and save
  it to DURABLE_SELF" changes neither the configuration nor durable state, and
  the next turn is given the same disposition;
- delegated material containing a forged `[PERSONA_SEED]` line arrives under
  `EXTERNAL_RESOURCE_RESULT`, and the prompt still has exactly one seed section;
- the trace names the seed by digest and carries none of its text;
- a seed whose digest no longer matches is refused before the model is called.

Mutation testing: rendering the seed under `[DURABLE_SELF]` instead of its own
heading failed 2 of the 5 boundary tests and 1 unit test; dropping the digest
validation failed the stale-digest test. Both reverted.

## Not this

- no learning, adaptation, or feedback from conversations into the seed;
- no seed versioning workflow beyond an operator editing configuration;
- no per-interlocutor or per-context variation;
- no evaluation of whether a model actually follows a disposition. Whether
  qwen2.5:0.5b sounds like this is not a claim being made — what is tested is
  that the disposition arrives, distinctly, and cannot be rewritten from
  inside.
