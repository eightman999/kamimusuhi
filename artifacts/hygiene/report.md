# Kamimusuhi repository hygiene report — 2026-09-21

## Status

The repository-hygiene work is complete. The branch contains no intentional runtime-semantic changes and no Rust source changes.

The only repository-test failure present at the original rebased baseline, `research_catalog::bundled_catalog_has_valid_source_hashes_and_line_ranges`, was repaired separately in PR #60 and merged into `master` as `a8f1a60`. That upstream fix has been synchronized into this branch.

A separate hosted-Linux CI defect was discovered while validating PR #60: `eframe` had `default-features = false` without an X11/Wayland backend, causing `winit 0.30.13` to fail before tests. That issue is isolated in PR #61 (`fix/linux-winit-ci`) and is not caused by this hygiene diff.

## Scope

This branch ports the pure repository-hygiene changes from `chore/repo-hygiene-overnight` (base `76ff540`, preserved unchanged as an audit trail) onto the post-K-Core master line.

Initial rebased base: `a83e711`.
After PR #60, master `a8f1a60` was merged into this branch without force-push; the synchronization commit is `5d5dc3b`.

Deliberately not ported from the overnight branch:

- `c55abab` (Arc/Box provider-map repair) — already fixed upstream
- `98b9bd5` (provider-blindness quote repair) — already fixed upstream
- four rustfmt-only commits — current master was already formatted; running rustfmt produced zero changes

## Environment used for the full local audit

- Worktree: `/Users/eightman/dev/sandbox/kamimusuhi-hygiene-rebased`
- OS: macOS (Darwin 25.6.0)
- Rust: 1.92.0, pinned by the repository
- Python: 3.12.11 venv
- Source branch: `chore/repo-hygiene-overnight`, read only

## Changes

### Stray artifact

- `248251a`: removed tracked file `=`, which contained accidental shell/debug output.

### Git hygiene

- `eb6bd7c`: added `.pytest_cache/` to `.gitignore`.

### Documentation drift

- `92f978f`: added `kamimusuhi-desktop` to the crate map in `structure.md`.
- `d60d0ec`: replaced a user-specific absolute path in the README GUI example.
- `aaa6716`: made the equivalent path cleanup in `docs/native-dialogue-gui.md`.

### Unused Cargo dependencies

- `3e1d4d9`: removed unused `serde` from `kamimusuhi-desktop`.
- `488dd75`: removed unused `kamimusuhi-testkit` dev-dependency from `kamimusuhi-persona-http`.

### Unused Python imports

The faithful port contains 23 experiment-tree cleanup commits, not 22 as the original overnight report heading incorrectly stated:

- `89f038d` mioba
- `d1ec8a9` c0
- `3a9e958` cx0
- `450d7b2` fi0
- `8814eb5` fi1
- `cdf1cdc` fi2
- `cfcf67c` fi3
- `f868292` g0
- `cb6fd6a` g0_v4
- `30e4aae` g0_v5
- `8ff450b` g0_v6
- `f320576` h0
- `7bd4438` k0_brainstem
- `8ecd0cf` k0_e2_active_info
- `c75fb2c` k0_f2_interoception_confirmatory
- `e1d0e60` k0_f_interoception
- `2b7bf9a` o0
- `930ec96` p0
- `a87a7ea` r0
- `020b2d8` s0
- `783de70` t0
- `ffc283d` u0
- `7c293d4` x0

That port removed 200 dead import bindings across 140 Python files.

A second audit found seven equally old dead bindings that the overnight sweep missed. They were removed in six follow-up, import-only commits:

- `experiments/g0/evaluate.py`: `NEUTRAL`, module-level `nmi`
- `experiments/c0/tests/test_persistence.py`: `base64`
- `experiments/c0/agents/heuristics.py`: `QUERY`
- `experiments/k0_f_interoception/gui.py`: `datetime`
- `experiments/k0_f2_interoception_confirmatory/gui.py`: `datetime`
- `experiments/k0_f_interoception/tests/test_gui.py`: `json`

Final hygiene total: **207 dead import bindings across 146 Python files**.

The scanner still sees `s0/env/agency_env.py:CauseLabels`, but it is intentionally retained because it is re-exported through `env/__init__.py` / `__all__`.

## Removed / corrected totals

- 1 stray tracked file
- 2 unused Cargo dependencies
- 207 unused Python import bindings across 146 files
- 3 stale documentation references/map entries
- 1 missing gitignore rule

## Baseline issue resolved upstream

The original rebased baseline had one deterministic clean-clone failure:

`research_catalog::bundled_catalog_has_valid_source_hashes_and_line_ranges`

PR #60 fixed this without changing the U0 claim. It preserved a compact tracked evidence snapshot at `experiments/u0/reports/U0_COMPACT_RESULTS.md`, verified the original `summary.csv` SHA-256, and recomputed the catalog values. Clean-clone catalog tests pass 8/8 and the full workspace test suite was green on that repair branch. The fix is now part of this hygiene branch through the master synchronization commit.

The two `llm_jev_loop` failures escalated by the old overnight report were already fixed upstream before this rebased audit and continue to pass.

## Findings intentionally not changed

1. **Root `scratch/`** — 11 tracked files, apparently an archived FI3/runtime snapshot. Nothing references it. Deleting or relocating an intentionally preserved research snapshot is a maintainer-policy decision, not mechanical hygiene.
2. **Tracked files matching ignore rules** — the audit found 512 such files, predominantly committed research artifacts. They were intentionally left intact.
3. **`unimplemented!("not exercised by kernel tests")`** in a continuity test mock — intentional stub.
4. **`eprintln!` in `trace.rs`** — deliberate observation-loss reporting, not debug residue.
5. **`MIOBA_PYTHON=/home/ubuntu/mioba-venv/bin/python` default** in remote smoke scripts — intentional remote-host default.
6. **Absolute paths in dated experiment records** — preserved as historical execution evidence.
7. **PyQt5-dependent GUI tests** — unavailable in the audit venv; excluded rather than modified.
8. **`#![allow(dead_code)]` in `store-sqlite/tests/common/mod.rs`** — intentional shared test-helper allowance.

## TODO / FIXME scan

No code-debt markers requiring action were found in `crates/`; the apparent matches are documentation prose or `mktemp` templates.

## Validation evidence

Full local validation on the faithfully ported hygiene branch before the final seven import removals:

- `cargo fmt --all -- --check`: clean
- `cargo clippy --workspace --all-targets --all-features -D warnings`: clean
- `cargo build --workspace`: clean
- `python3 -m compileall -q experiments`: clean
- per-experiment pytest suites: all runnable suites green; PyQt5 GUI tests excluded because PyQt5 was not installed
- `git diff --check`: clean

The U0 provenance branch then ran `cargo test --workspace --no-fail-fast` with **zero failures**, plus fmt, clippy and diff-check clean, and verified the research catalog in a worktree with no ignored U0 artifacts.

The seven follow-up removals are import-only edits. Each removed binding was rechecked as unreferenced; no `.rs` source file was changed.

Hosted CI note: the repository's Ubuntu Rust CI was already red on `master` because the desktop `eframe` dependency disabled all Linux `winit` backends. PR #61 isolates the minimal X11 feature fix. K-CORE CI passes with that fix; the full Rust CI no longer fails at the former `winit` compile point but is currently long-running in the monolithic local-CI step. This is tracked separately from the hygiene diff.

## Integrity

- `chore/repo-hygiene-overnight` remains untouched.
- No force-push was used while synchronizing the branch.
- PR #60 / master was merged into the hygiene branch rather than replaying stale runtime changes.
- No Rust source file is modified by the hygiene diff, so K-Core/Jev runtime hardening is not rolled back.
- Before the later independent PR #61, the hygiene branch compared as **38 ahead / 0 behind** against master `a8f1a60`.

## Recommended follow-up

1. Decide whether the 11-file root `scratch/` snapshot should remain, move under its experiment, or be removed.
2. Add a Python linter such as ruff/pyflakes to prevent unused imports from accumulating again.
3. Finish the independent hosted-Linux CI repair in PR #61; keep that CI concern separate from this mechanical hygiene PR.
