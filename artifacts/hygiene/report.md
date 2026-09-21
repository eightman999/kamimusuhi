# REVIEW REQUIRED

One unresolved item needs maintainer review: two `llm_jev_loop` tests fail
deterministically on master (masked until now because the workspace did not
compile). Everything else is mechanical cleanup.

# Kamimusuhi repository hygiene report — 2026-09-21

## Environment

- Base commit: `76ff540` (origin/master)
- Cleanup branch: `chore/repo-hygiene-overnight`
- Worktree: `/Users/eightman/dev/sandbox/kamimusuhi-hygiene` (dedicated git
  worktree; the user's worktree at `../kamimusuhi` was never modified)
- OS: macOS (Darwin 25.6.0); Rust 1.92.0 (pinned toolchain); Python 3.12 venv
  reused read-only for pytest runs
- Gate: `scripts/ci-local.sh` = fmt --check, clippy -D warnings, cargo test,
  core dependency boundary

## Baseline (at origin/master, before any change)

Master was **broken** — the workspace did not compile, so CI could not have
been green:

1. `dialogue.rs:321` — `Box::new(provider)` inserted into
   `BTreeMap<String, Arc<dyn LanguageProvider>>` (missed call site when the
   field migrated to Arc in the "Arc race semantics" series).
2. `llm_jev.rs:1687-1688` — syntax error `contains(""fixture"")` in a test
   module (introduced by `3b70686`).

Baseline repairs (required to verify anything; minimal, single-resolution
fixes, one commit each):

- `c55abab` fix(dialogue): annotate provider map so extra providers coerce to
  `Arc<dyn LanguageProvider>` — `let mut language_providers: BTreeMap<String,
  Arc<dyn LanguageProvider>>` + `Arc::new(provider)`.
- `98b9bd5` test(jev): fix doubled quotes in provider-blindness assertions —
  `contains("\"fixture\"")` / `contains("\"mock\"")`.

After repairs: build clean, clippy clean (0 warnings), `cargo fmt --check`
still failing on 4 files with pre-existing drift.

## Changes (36 commits total, all small and independently revertable)

### Baseline repairs (2)
- `c55abab`, `98b9bd5` — described above.

### Stray artifacts
- `03941fa` chore: remove stray shell output accidentally committed as file
  `=` — captured debug output (`first_xid79_line=`, `=== all AER/Xid/NVRM
  ===`) committed in `4b338d4`; unreferenced anywhere.

### Formatting drift (4)
- `f609285` `d8341d2` `1769748` `51ffc02` style: apply rustfmt to
  `llm_jev/assessment.rs`, `dialogue_setup.rs`, `llm_jev_loop.rs`,
  `desktop/app.rs` — committed code failed `cargo fmt --check`; pure
  reflow/indent, no semantic change. Gate now green.

### Unused dependencies (2)
- `678ab5f` chore(desktop): drop `serde` (only `serde_json` is used).
- `4ec1524` chore(persona-http): drop `kamimusuhi-testkit` dev-dep (no tests/
  dir; unit tests use only kamimusuhi-core).

### Git hygiene (1)
- `58862e3` chore: ignore `.pytest_cache` — pytest run state was untracked
  but not ignored.

### Documentation drift (3)
- `9c737af` docs: add `kamimusuhi-desktop` to structure.md crate map (7
  workspace members; map listed 6).
- `79e01be` docs: replace user-specific absolute paths in README GUI example
  (`/Users/eightman/...` → repo-relative, matching surrounding convention).
- `8d261f7` docs: same fix in `docs/native-dialogue-gui.md` (the doc linked
  from the README section).

### Unused imports (22 commits, one per experiment tree)
- `36f2e2a` mioba — 34 files; verified: compileall + `pytest
  experiments/mioba/tests` 667 passed.
- `300d1c7` c0 (17 passed), `85d167c` cx0 (26), `8f26d37` fi0 (58),
  `77c391e` fi1 (49), `bff9810` fi2 (no tests), `3ffbb3a` fi3 (135),
  `c71a09d` g0 (24), `f0ec6d4` g0_v4 (10), `19c9c71` g0_v5 (20),
  `1181838` g0_v6 (6), `b591d09` h0 (27), `21489de` k0_brainstem (21),
  `54ad788` k0_e2_active_info (47), `6b3c848` k0_f2 (38), `5754d06` k0_f
  (46), `918fe24` o0 (27), `cf6d41a` p0 (26), `1d2cc72` r0 (19), `9d0bda4`
  s0 (14), `0c63fe2` t0 (47), `191a496` u0 (44), `dadfdbb` x0 (24).

Method: AST scan for import-bound names occurring exactly once in the file;
removed names from `from X import (...)` lists or deleted whole lines when
all names were dead. Skipped `__init__.py` and `__future__`; names
re-exported by a package `__init__.py` (via `from .mod import name` or
`__all__`) were kept — this caught `CauseLabels` in `s0/env/agency_env.py`,
which is re-exported through `env/__init__.py` despite being unused inside
the module itself. Post-check confirmed zero removed names are imported
elsewhere.

## Findings not changed (VERIFY / DO NOT TOUCH)

1. **`llm_jev_loop::accept_never_bypasses_an_unsuitable_selected_candidate`
   fails deterministically** — `judge.request_count() <= 3` exceeded; Jev is
   invoked more times than the scripted fixture allows. The test was added in
   `564b361` (feat: evidence-bound Jev assessment) *before* the
   race/rejudge semantics commits (`070ac27`, `4b832e1`, `d3a5bc7` …). Either
   the test expectation is stale relative to intended rejudge semantics, or
   the implementation over-calls Jev. Semantic — needs maintainer decision.
2. **`llm_jev_loop::retry_replaces_one_candidate_and_reselects_without_a_third_generation`
   fails deterministically** — same class: judge consumes more scripted
   responses than provided ("TypeSafe batch failed validation after one
   retry").
3. **`research_catalog::bundled_catalog_has_valid_source_hashes_and_line_ranges`
   fails on any clean clone** — `knowledge/experiment-findings.json` entry
   `u0-need-guided-memory` cites `experiments/u0/artifacts/results/summary.csv`,
   which is gitignored (`/experiments/u0/artifacts/`). The digest/line-range
   contract cannot hold for untracked sources. Options: commit a stable
   source copy, or point the source at a tracked file. Semantics — not
   touched.
4. **`scratch/` at repo root** — 11 tracked files (fake runtime `rt/` +
   `state/` JSON) committed in `13351b1` "wip(fi3) snapshot ... preserved
   before worktree disposal". Nothing references it; it looks like generated
   runtime state archived deliberately. Left in place — deleting an archived
   snapshot is a judgment call for the maintainer.
5. **512 tracked files that match `.gitignore` rules** — experiment artifacts
   (`experiments/*/artifacts/`, logs, `.pt`/`.png` results) committed before
   or despite ignore rules. They are the research record; intentionally kept.
6. **`unimplemented!("not exercised by kernel tests")`** in
   `continuity.rs` test mock — intentional stub methods.
7. **`eprintln!` in `trace.rs`** — deliberate "observation lost" error
   reporting, documented by its comment. Not debug logging.
8. **`MIOBA_PYTHON` default `/home/ubuntu/mioba-venv/bin/python`** in
   `experiments/mioba/scripts/smoke_*.sh` — intentional default for the
   remote GPU host; CI overrides via env. Not drift.
9. **Absolute paths inside dated experiment records** (`PREFLIGHT_AUDIT.md`,
   `EXPERIMENT_LINEAGE.md`, `s0/README.md`, `PERSONA_RESEARCH_REPORT.md`) —
   historical evidence of what was run; rewriting records is out of scope.
10. **PyQt5-dependent GUI tests** (`k0_e2`, `k0_f`, `k0_f2`
    `tests/test_gui.py`) — cannot collect without PyQt5 on this machine;
    environmental, untouched.
11. **`#![allow(dead_code)]` in `store-sqlite/tests/common/mod.rs`** —
    shared test helper module; intentional.

## Remaining TODO / FIXME

None found in code. `crates/` has zero `TODO|FIXME|HACK|XXX|DEPRECATED`
markers; the only `TODO`-regex hits were `mktemp -d XXXX` templates and
prose in docs. `docs/persistent-agent-implementation-pitfalls.md` contains
advisory notes, not code debt.

## Existing failures

The three deterministic test failures listed in "Findings" existed at base
(latent behind the compile errors). No new failures were introduced.

## New failures

None. Zero observed after all changes.

## Final validation

- `cargo fmt --all -- --check`: clean
- `cargo clippy --workspace --all-targets --all-features -D warnings` path of
  `ci-local.sh`: clean (0 warnings)
- `cargo build --workspace`: clean
- `cargo test --workspace --no-fail-fast`: all targets pass except the 3
  pre-existing failures above
- `python3 -m compileall -q experiments`: clean
- pytest per experiment tree: all green (GUI tests excluded — no PyQt5)
- `git diff --check`: clean
- Core dependency boundary check (ci-local.sh): passes

## Recommended follow-up (separate tasks)

1. Reconcile the two `llm_jev_loop` tests with the intended Jev
   race/rejudge call budget — decide whether tests or implementation drifted.
2. Fix the research catalog's reference to a gitignored artifact (commit a
   stable source, or re-point the provenance record).
3. Decide whether root `scratch/` should be deleted, moved under
   `experiments/fi3/`, or kept as an archive.
4. Consider installing a Python linter (ruff/pyflakes) in the mioba CI gate —
   ~100 unused imports had accumulated unnoticed.
5. `docs/` README index could gain a check for doc→doc links (currently zero
   broken — keep it that way).
