# FI2・FI3 integration (2026-11-08)

## Contract

FI2 validates capture/summary drafts at central intake before activation. Accepted forms are complete extractive source texts joined chronologically (timestamp, evidence ID), or structured claims whose fields exactly match cited source claims. Legacy count-only summaries count distinct raw user roots. General paraphrase correctness is not inferred: unverifiable drafts are rejected. Quotes must cover all referenced sources. Negation, scope, certainty, sequence and validity bounds cannot be silently changed. Invalid or unpresented references reject the whole draft; normalization cannot rescue them. Nested metadata is rejected; recall metadata belongs at the outer level, where typed presented-reference checks apply.

Evidence remains immutable. DerivedFrom lineage and correction propagation remain authoritative; a self-reported fidelity flag cannot bypass source traversal. Non-summary facts and corrections still use existing kernel mutation rules. This is a conservative fidelity boundary, not a general natural-language entailment solver.

FI3 fingerprints declared models, prompts, sampling/tool settings and compiled prompt implementation. Credentials and mutable health/counters are excluded. Remote weight bytes are unknown, not certified. New individuals enroll a baseline. Existing individuals without one require explicit review; read-only inspection still works. Corrupt or incompatible manifests fail closed. Changes to on-disk configuration and stale warm sessions are checked separately.

Turns, reflection and central intake hold a shared identity-use lease. Acceptance takes an exclusive lease and uses an exact review token, so stale approvals cannot silently authorize a different candidate. Model/prompt changes detected during generation prevent delivery and canonical writes.

Resident supplies effective local/peer declarations each turn. Pins bind requests to the reviewed declaration and peer route snapshot before inference. The HTTP Persona checks the effective routed model before interpreting tool calls or output. Old peers without declarations cannot receive pinned inference. Direct external API weight changes or dishonest model labels remain unverifiable.

## Existing individual migration

1. Update all participating resident nodes together; do not infer acceptance from a successful restart.
2. Obtain the authenticated `/v1/identity-context` response for the intended resident and save it locally (contains declaration digests, no credentials).
3. Run `kamimusuhi-runtime identity-status --dir <individual> --identity-context-file <context.json>`.
4. Review changed components, then run `identity-accept` with the same directory/context and `--review-token <exact token>` returned by status.
5. Recreate warm dialogue hosts after acceptance. A host retaining the old declaration is deliberately refused.

When an upstream returns a different model label from its configured request alias, declare the reviewed response name explicitly in the resident tier’s `identity_model_aliases` list. The list is bounded, fingerprints are order independent, and changes require renewed identity review on affected nodes. Unknown response labels remain rejected; no response teaches the allowlist automatically.

Never add automatic acceptance to startup or fallback handling. A configured fallback inside the accepted model set continues normally; a newly declared model requires review.

## Verification

Local deterministic fixtures exercise FI2 semantics, provenance/correction recall, FI3 migration/CAS, warm sessions and in-flight changes, and resident HTTP pin propagation. Real LLM semantics, production replacement and remote model weight integrity are not verified by these tests. No deployment is part of this change.

Final command: `cargo test -p kamimusuhi-runtime -p kamimusuhi-persona-http -p kamimusuhi-resident --offline` (local fixture TCP/process permissions enabled): **690 passed, 0 failed, 6 ignored**, exit 0. Includes FI2 14, FI3 integration 11, HTTP Persona 57 and resident unit 233 tests. Ignored cases retain existing opt-in/child-scenario requirements; they are not counted as passed. Provider permanent-stop subprocess regression tests also passed.

`cargo fmt` check for the four affected packages, `git diff --check`, and Clippy for runtime/persona-http/resident libraries with `-D warnings` passed. External critique MCP was attempted with user authorization, but returned unavailable (exit 1); it supplied no review evidence. Independent local review found and closed dynamic provider declaration, endpoint canonicalization and nested metadata/reference bypasses.

Production follow-up: explicit response-model aliases were added after a correctly refused production alias mismatch. The updated three-crate regression run passed **691 tests, 0 failed, 6 ignored**. See [production deployment](p1-production-deployment-2026-11-08.md) for the reviewed alias, rollback, final deployment and actual verification boundaries.
