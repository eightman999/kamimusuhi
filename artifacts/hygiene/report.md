# REVIEW REQUIRED

One pre-existing failure remains on master and is carried into this branch
unchanged: `research_catalog::bundled_catalog_has_valid_source_hashes_and_line_ranges`
(catalog cites a gitignored artifact — needs a maintainer decision on the
provenance contract). The hygiene port itself introduces zero new failures
and zero semantic changes.

# Kamimusuhi repository hygiene report (rebased) — 2026-09-21

## Scope

Port of the pure repository-hygiene changes from
`chore/repo-hygiene-overnight` (base `76ff540`, kept unmodified as audit
trail) onto current `origin/master` (`a83e711`). Deliberately NOT ported:

- `c55abab` (Arc/Box provider-map repair) — already fixed upstream
- `98b9bd5` (provider-blindness quote repair) — already fixed upstream
- the four rustfmt-only commits — `cargo fmt` was run against current code
  instead and produced **zero changes** (master is already formatted)

## Environment

- Base commit: `a83e711` (origin/master)
- Branch: `chore/repo-hygiene-rebased`
- Worktree: `/Users/eightman/dev/sandbox/kamimusuhi-hygiene-rebased`
- OS: macOS (Darwin 25.6.0); Rust 1.92.0 (pinned); Python 3.12.11 venv

## Baseline (see baseline.md)

Master builds clean; fmt/clippy clean; one pre-existing test failure
(`research_catalog` provenance). The two `llm_jev_loop` tests that the
previous report escalated now pass at base.

## Changes — 30 commits, all cherry-picked from the overnight branch

Each cherry-pick applied cleanly; no conflicts, no manual merge resolution.
After every pick, the removed import-bound names were re-verified as
unreferenced in the post-commit tree (AST + textual scan).

### Stray artifacts (1)
- `248251a` remove `=` (captured shell output) — still tracked on master;
  deleted.

### Git hygiene (1)
- `eb6bd7c` ignore `.pytest_cache/` — still absent from `.gitignore` on
  master; added.

### Documentation drift (3)
- `92f978f` structure.md: add `kamimusuhi-desktop` to crate map (workspace
  still has 7 members; map still listed 6)
- `d60d0ec` README: repo-relative paths in GUI example (still
  `/Users/eightman/...` on master)
- `aaa6716` docs/native-dialogue-gui.md: same class (still absolute paths)

### Unused dependencies (2)
- `3e1d4d9` drop `serde` from `kamimusuhi-desktop` — still declared, still
  unreferenced (re-grepped on master)
- `488dd75` drop `kamimusuhi-testkit` dev-dep from `kamimusuhi-persona-http`
  — still declared, still unused (no tests dir; unit tests use only
  kamimusuhi-core)

### Unused Python imports — 23 experiment-tree commits (not 22)

The previous report's heading said "22 commits" but enumerated 23 trees;
the correct count is **23**:

- `89f038d` mioba (34 files), `d1ec8a9` c0, `3a9e958` cx0, `450d7b2` fi0,
  `8814eb5` fi1, `cdf1cdc` fi2, `cfcf67c` fi3, `f868292` g0, `cb6fd6a`
  g0_v4, `30e4aae` g0_v5, `8ff450b` g0_v6, `f320576` h0, `7bd4438`
  k0_brainstem, `8ecd0cf` k0_e2_active_info, `c75fb2c`
  k0_f2_interoception_confirmatory, `e1d0e60` k0_f_interoception, `2b7bf9a`
  o0, `930ec96` p0, `a87a7ea` r0, `020b2d8` s0, `783de70` t0, `ffc283d`
  u0, `7c293d4` x0.

Totals on this branch: 140 Python files, 200 import-bound names removed —
identical counts to the overnight branch, confirming a faithful port.

Post-port verification (all clean):
- zero removed name is referenced in its own file (AST names, attributes,
  string annotations, argument names, raw text)
- no cross-module re-export broken (2 apparent hits were verified false
  positives: `eval_dynfeat` is imported from `experiments/g0/evaluate.py`,
  where it remains defined — the removal was g0_v4's own unused import)
- no `mock.patch` / `monkeypatch.setattr` string target hits a removed name
- no `__init__.py` modified; `__future__` imports untouched; imports inside
  `try/except ImportError` untouched
- every changed hunk is import-related (verified mechanically)

## Removed

- 1 stray tracked file (`=`)
- 2 unused Cargo dependencies (`serde` in desktop; `kamimusuhi-testkit`
  dev-dep in persona-http)
- 200 unused Python import bindings across 140 files
- 3 stale doc references (user-specific absolute paths; missing crate in
  map)
- 1 missing gitignore rule (`.pytest_cache/`)

## Findings re-verified on latest master — not changed

1. **`research_catalog::bundled_catalog_has_valid_source_hashes_and_line_ranges`
   still fails deterministically** — provenance entry cites a gitignored
   artifact. Same as previous report; still needs maintainer decision.
2. **`scratch/` at repo root** — still present, 11 tracked files (fake
   runtime `rt/` + `state/`). Still nothing references it; deliberately
   left as an archived snapshot decision for the maintainer.
3. **512 tracked files match `.gitignore` rules** — unchanged; research
   record kept intentionally.
4. **`unimplemented!("not exercised by kernel tests")`** in
   `continuity.rs` test mock — still present; intentional stub.
5. **`eprintln!` in `trace.rs:200`** — still present; deliberate
   "observation lost" reporting.
6. **`MIOBA_PYTHON` default `/home/ubuntu/mioba-venv/bin/python`** in
   mioba `smoke_*.sh` — still present; intentional remote-host default.
7. **Absolute paths inside dated experiment records** — unchanged; not
   drift, historical evidence.
8. **PyQt5-dependent GUI tests** — still cannot collect in this
   environment; untouched.
9. **`#![allow(dead_code)]` in `store-sqlite/tests/common/mod.rs`** —
   intentional shared-helper allowance.

## New residual findings (present on master AND on the old base — the
overnight branch left them too; recorded, not removed, to keep this a
faithful port)

A post-port AST scan found 7 genuinely dead imports remaining (the scan
also flags `s0/env/agency_env.py:CauseLabels`, which is a re-export through
`env/__init__.py` `__all__` — false positive, correctly kept):

- `experiments/g0/evaluate.py`: `NEUTRAL` (l.43), `nmi` (l.46; module-level
  import redundant with the local `nmi as nmi_fn` at l.238)
- `experiments/c0/tests/test_persistence.py`: `base64` (l.1)
- `experiments/c0/agents/heuristics.py`: `QUERY` (l.12)
- `experiments/k0_f_interoception/gui.py` and
  `k0_f2_interoception_confirmatory/gui.py`: `datetime` (l.10)
- `experiments/k0_f_interoception/tests/test_gui.py`: `json` (l.2)

All seven are in files identical between the old base and current master —
they were equally dead on the overnight branch. Candidates for a follow-up
sweep, not blockers.

## Remaining TODO / FIXME

None in code (unchanged from previous audit — `crates/` has zero markers).

## Existing failures

`research_catalog::bundled_catalog_has_valid_source_hashes_and_line_ranges`
— deterministic at base and after port; unrelated to any change here.

## New failures

None.

## Final validation (all on this branch)

- `cargo fmt --all -- --check`: clean (also: `cargo fmt` run — zero output
  changes, so no fmt commit exists on this branch)
- `cargo clippy --workspace --all-targets --all-features -D warnings`: clean
- `cargo build --workspace`: clean
- `cargo test --workspace --no-fail-fast`: all pass except the one
  pre-existing catalog failure; the two `llm_jev_loop` tests pass (19/19)
- `python3 -m compileall -q experiments`: clean
- pytest per experiment tree: all green — c0 17, cx0 26, fi0 58, fi1 49,
  fi3 135, g0 24, g0_v4 10, g0_v5 20, g0_v6 6, h0 27, k0_brainstem 21,
  k0_e2 47, k0_f 46, k0_f2 38, mioba 667, o0 27, p0 26, r0 19, s0 14,
  t0 47, u0 44, x0 24 (fi2 has no tests; GUI tests env-excluded)
- `git diff --check`: clean
- No `.rs` file modified — zero runtime-semantics diff

## Integrity checks

- Branch is based on `origin/master` `a83e711`; overnight branch untouched
- Working tree clean; no force-push; no remote branch changes
- No `.rs` changes → impossible to have rolled back K-Core/Jev hardening
- Every ported commit re-verified against current code (not blind replay)

## Recommended follow-up (separate tasks)

1. Fix the research catalog provenance entry (`u0-need-guided-memory` →
   gitignored artifact): commit a stable source copy or re-point the
   provenance record.
2. Decide `scratch/` disposition (delete / move under `experiments/fi3/` /
   keep as archive).
3. Sweep the 7 residual dead imports listed above.
4. Consider adding a Python linter (ruff/pyflakes) to the mioba CI gate.
