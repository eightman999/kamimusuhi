# Hygiene baseline — 2026-09-21

## Environment

- Base commit: `76ff540` (origin/master)
- Cleanup branch: `chore/repo-hygiene-overnight`
- Worktree: `/Users/eightman/dev/sandbox/kamimusuhi-hygiene`
- OS: macOS (Darwin 25.6.0)
- Toolchain: rustc/cargo 1.92.0 (pinned via rust-toolchain.toml)

## Baseline gate results at base commit (before any change)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | FAIL — drift in `kamimusuhi-desktop/src/app.rs`, `kamimusuhi-runtime/src/dialogue_setup.rs`, `kamimusuhi-runtime/src/llm_jev/assessment.rs`, `kamimusuhi-runtime/tests/llm_jev_loop.rs` |
| `cargo build --workspace` | FAIL — `dialogue.rs:321` type error (`Box` inserted into `BTreeMap<String, Arc<dyn LanguageProvider>>`) |
| `cargo clippy --workspace --all-targets --all-features` | FAIL — same compile error, plus syntax error in `llm_jev.rs` test module (`""fixture""`) |
| `cargo test --workspace` | Could not run fully at base (compile errors above) |

## Baseline repairs (required before verification was possible)

- `c55abab` fix(dialogue): annotate provider map so extra providers coerce to `Arc<dyn LanguageProvider>` — compile error introduced when the field migrated to `Arc` (test-side updated in `d3a5bc7`, production call site missed).
- `98b9bd5` test(jev): fix doubled quotes in provider-blindness assertions — `""fixture""` syntax error introduced in `3b70686`.

## Existing failures after baseline repairs

Deterministic, reproduce on every run. Latent at base — masked because the
workspace did not compile.

1. `llm_jev_loop::accept_never_bypasses_an_unsuitable_selected_candidate`
   — panics at `llm_jev_loop.rs:598` (`judge.request_count() <= 3` exceeded;
   Jev is invoked more times than the scripted fixture allows).
2. `llm_jev_loop::retry_replaces_one_candidate_and_reselects_without_a_third_generation`
   — `Err(Conversation(Malformed("TypeSafe batch failed validation after one retry")))`;
   judge consumes more scripted responses than the test provides.
3. `research_catalog::bundled_catalog_has_valid_source_hashes_and_line_ranges`
   — `knowledge/experiment-findings.json` source `u0-need-guided-memory`
   references `experiments/u0/artifacts/results/summary.csv`, which is
   gitignored (`/experiments/u0/artifacts/`). The file exists only in local
   checkouts that ran the experiment; any fresh clone fails this test.

## Green at baseline (after repairs)

- clippy: clean, zero warnings
- All other test targets pass (core 107, runtime lib 63, store-sqlite suite,
  persona/resource http, desktop, all other integration tests)
