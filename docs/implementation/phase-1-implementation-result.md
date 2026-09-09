# Phase 1 implementation result — v0.1 continuity slice

What the first implementation phase (W0–W5) actually built, what it
demonstrated, and what it did not. This is a record of measured outcomes, not
a plan. Where something is unproven or absent it says so.

- Closing commit: `790fcd1`
- Issue: [#8](https://github.com/eightman999/kamimusuhi/issues/8) — v0.1 vertical slice
- Wave issues: #15 (W2), #16 (W3), #17 (W4) — all closed
- Reproduce: `./scripts/demo-v0.1.sh` and `./scripts/ci-local.sh`

---

## 1. The claim

> A persistent self exists outside the context window and outside replaceable
> cognitive resources, remains inspectable, and can resume cognition after
> runtime replacement.

This was demonstrated. The demonstration is narrow on purpose: it is a claim
about **identity and provenance boundaries**, not about intelligence,
autonomy, or artificial life. Nothing here shows that Kamimusuhi thinks well.
It shows that whatever thinks is not the same thing as who it is.

---

## 2. What each wave established

| Wave | Established | Where |
|---|---|---|
| W0 | Cargo workspace, provider-neutral core, dependency boundary enforced in CI | `crates/`, `scripts/ci-local.sh` |
| W1 | Canonical continuity: single writer, atomic activation, writer-epoch fencing, stale-predecessor rejection, restart recovery, FP01–FP07 crash windows | `kamimusuhi-core::continuity`, `kamimusuhi-store-sqlite` |
| W2 | Canonical evidence separate from derived memory; episodic + relationship state; correction/supersession without destructive rewrite; transitive invalidation; structural domain separation | `core::evidence`, `core::memory`, `core::domain_separation` |
| W3 | Library with provenance; typed attributed workspace; cognitive-resource registry; deterministic Fake A/B; resource-call attribution | `core::library`, `core::workspace`, `core::resources` |
| W4 | `init` / `inspect` / `demo-continuity`; restart across real PIDs; JSONL operational trace separate from canonical audit | `kamimusuhi-runtime` |
| W5 | Real HTTP adapter; OpenAI-compatible provider; timeout/retry/error classification; logical call vs physical attempt; no stored secrets | `kamimusuhi-resource-http` |

---

## 3. Issue #8 acceptance, item by item

| Criterion | Result | Evidence |
|---|---|---|
| Reproducible from a clean checkout | met | `scripts/demo-v0.1.sh`; `the_scenario_is_reproducible_from_a_clean_directory` |
| No previous-session prompt buffer after restart | met | `restart_across_processes_restores_the_same_individual_without_a_chat_buffer` — child process gets a directory path, stdin closed, no conversation text in argv |
| Canonical lineage/self survives termination | met | same test: same `IndividualId`, same root commit, head restored at the generation process A left |
| Resource replacement does not change the identity record | met | `replacing_fake_a_with_fake_b_changes_the_material_and_not_the_individual`; `replacing_a_provider_across_a_restart_changes_material_and_not_identity` |
| Self state and Library evidence are separate trace/workspace sections | met | workspace: distinct `WorkspaceDomain` + `SourceRef` variants; trace: `memory.retrieved` vs `library.retrieved` as distinct events with distinct correlation |
| External model output attributed as delegated cognitive material | met | `EXTERNAL_RESOURCE_RESULT` with `authority = external_material`, plus a `resource_calls` row naming the resource that actually answered |
| Passes with deterministic fakes | met | Fake A/B in `kamimusuhi-testkit`; the whole demo runs fake-only |
| At least one real model adapter can run the same basic flow | met, with a stated limit | `OpenAiCompatibleResource` runs the full flow over real TCP in `provider_vertical_slice.rs`. The endpoint is a local fixture server, and the adapter is plain HTTP — see §5 |
| README command or script documents the demo | met | `scripts/demo-v0.1.sh`, documented in `README.md` and `README.en.md` |

The optional K-Edge → K-Core extension was **not** attempted. It is not part
of the acceptance criteria.

---

## 4. Measured outcome of the v0.1 scenario

One run of `scripts/demo-v0.1.sh` (deterministic seeds 1 / 10 / 20):

```text
init      individual …0003, root commit …0004, generation 0
process A pid P₁, boot B₁
          evidence → proposal → ACCEPTED → commit …0009, generation 1
          Library imported (3 chunks), Fake A → "result-a"
          workspace: CURRENT_CONTINUITY_STATE, CURRENT_INPUT,
                     RELATIONSHIP_MEMORY, LIBRARY_EVIDENCE,
                     EXTERNAL_RESOURCE_RESULT
          exit
process B pid P₂ ≠ P₁, boot B₂ ≠ B₁
          same individual …0003, head restored at generation 1
          prior_context: none
          relationship memory restored: {"preference":"ほうじ茶"}
          Library retrieved, not re-imported (same artifact and chunk id)
          Fake B → "result-b", resource id changes, head does not
          head_before == head_after
inspect   read-only; row counts and writer epoch unchanged across two runs
```

Test suite: **215 passing, 0 failed, 0 ignored** (core 68, store-sqlite 64,
resource-http 35, testkit 15, runtime 33; one of those is a child-process
re-run of the environment-variable case).

---

## 5. Known limits and risks

Stated plainly, because an implementation record that only lists successes is
not a record.

**The "real adapter" claim is bounded.** The OpenAI-compatible adapter speaks
real HTTP over a real socket and is exercised against a local fixture server.
It has **no TLS**, so it cannot reach `https://` providers; an `https://` base
URL is refused at configuration time. It has never been run against a hosted
commercial model. What is proven is that the adapter boundary works and that
provider failure does not reach identity — not that any particular provider
works.

**Persona Core is a deterministic fixture, not a model.** `FakePersonaCore`
reproduces the runtime contract; it does not think. Everything about response
quality, persona, and learning is untouched.

**The self domain is reserved, not implemented.** No operation accepts
`MutationDomain::SelfModel`. Outside evidence is rejected as
`SELF_DOMAIN_CONTAMINATION`, which is a boundary, not a self-model.

**No belief layer.** State records depend on other state only through evidence
lineage. There is no state→state or Claim→Belief dependency graph, so a belief
derived from other beliefs cannot be expressed.

**Retention is declared, not enforced.** `RetentionClass` is stored and acted
on by nothing. The deletion closure of audit A04 does not exist.

**Single writer only.** Writer-epoch fencing is the phase-1 stand-in for node
fencing. Multi-writer, distributed handoff, and split-brain protection are
absent, and the eager-invalidation design has concurrency implications that
only matter once multi-writer exists.

**Retrieval is lexical.** Library retrieval scans candidate chunks and scores
by matched terms; memory retrieval is an indexed lookup. No embeddings, no
semantic search, no learned ranking. Fine at fixture volume; unmeasured at any
real corpus size.

**No routing.** One slot, one implementation, chosen by configuration. There
is no capability metadata, no privacy-constrained routing, and no failover.

**Trace retention is one rotated generation.** At 8 MiB the trace rotates and
keeps one previous file. Anything that must survive belongs in the database —
which is why `resource_calls.turn_id` exists.

**ID seeds are operator-managed.** Two processes given the same deterministic
seed fail closed before writing, but nothing allocates seeds for them.

**Not demonstrated at all:** autonomous or background cognition, goals,
sleep/dream/replay, interoception, speech/ASR/TTS, sensors, actuators, an
Executor, K-Nerve, K-Edge/K-Core, private or online weight learning,
multi-individual orchestration.

---

## 6. What v0.1 does not mean

Closing #8 does not claim autonomous development, consciousness, artificial
life, or mature distributed embodiment. It claims one thing: the identity and
provenance boundaries hold under process termination and resource
replacement, and they hold structurally — by origin, ownership and domain —
rather than by inspecting what any text says.
