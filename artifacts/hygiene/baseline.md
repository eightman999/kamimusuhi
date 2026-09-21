# Hygiene baseline (rebased) — 2026-09-21

## Environment

- Base commit: `a83e711` (origin/master, fetched 2026-09-21)
- Cleanup branch: `chore/repo-hygiene-rebased`
- Worktree: `/Users/eightman/dev/sandbox/kamimusuhi-hygiene-rebased`
- Source branch being ported: `chore/repo-hygiene-overnight` (left
  unmodified as audit trail)
- OS: macOS (Darwin 25.6.0)
- Toolchain: rustc/cargo 1.92.0 (pinned via rust-toolchain.toml);
  Python 3.12.11 via `../kamimusuhi/.venv` (read-only pytest runs)

## Baseline gate results at base commit (before any change)

Unlike the previous overnight run, current master **compiles cleanly** —
the Arc/Box mismatch and the `""fixture""` syntax error were fixed upstream
(`aba9f19` series), and PR #59 (K-Core dialogue hardening) is merged.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS — no drift |
| `cargo clippy --workspace --all-targets --all-features -D warnings` | PASS — 0 warnings |
| `cargo build --workspace` | PASS |
| `cargo test --workspace --no-fail-fast` | 1 failure (pre-existing, see below) |
| `python3 -m compileall -q experiments` | PASS |

## Pre-existing failure at base (verified)

1. `research_catalog::bundled_catalog_has_valid_source_hashes_and_line_ranges`
   — `knowledge/experiment-findings.json` entry `u0-need-guided-memory`
   cites `experiments/u0/artifacts/results/summary.csv`, which is gitignored
   (`/experiments/u0/artifacts/`). Deterministic failure on any clean
   checkout. Unchanged from the previous report; still a provenance-design
   question, not touched.

## Previously failing — now passing at base

- `llm_jev_loop::accept_never_bypasses_an_unsuitable_selected_candidate` —
  PASS (19/19 tests in suite)
- `llm_jev_loop::retry_replaces_one_candidate_and_reselects_without_a_third_generation`
  — PASS

Both were reconciled upstream with the intended Jev rejudge semantics.

## Environmental limitation

PyQt5 is not installed in the available venv; `test_gui.py` in
`k0_e2_active_info`, `k0_f_interoception`, and
`k0_f2_interoception_confirmatory` cannot be collected. Same limitation as
the previous run; tests were excluded, not modified.
