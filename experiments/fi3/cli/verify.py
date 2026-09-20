"""``verify`` CLI — compare the current runtime against the stored
baseline manifest, classify the drift, and enforce continuation rules.

Usage::

    python -m experiments.fi3.cli.verify --runtime-dir R --state-dir S
    python -m experiments.fi3.cli.verify --runtime-dir R --state-dir S --accept-review

Gate semantics (``ContinuationGate``):

* IDENTICAL / SAFE change  -> continuation allowed, baseline updated
* REVIEW                   -> denied unless ``--accept-review``; on
  acceptance the baseline is updated to the current manifest
* BREAKING                 -> denied, state -> ``MIGRATION_REQUIRED``,
  a migration record (old manifest, new manifest, diff, classification)
  is written for the future Fi4 migrator; the baseline stays pinned

Exit codes: 0 continuation allowed / baseline initialized; 2 REVIEW
acceptance required; 3 BREAKING (state is MIGRATION_REQUIRED); 4 usage
or integrity error.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional

from ..identity.diff import diff_manifests
from ..identity.manifest import (
    collect_manifest,
    missing_required_fields,
    runtime_identity_hash,
)
from ..policies.compatibility import (
    BREAKING,
    IDENTICAL,
    REVIEW,
    SAFE,
    ClassificationReport,
    classify_changes,
)
from ..repro import run_metadata
from ..state import (
    STATUS_MIGRATION_REQUIRED,
    STATUS_OK,
    load_state,
    record_migration,
    save_state,
)

EXIT_OK = 0
EXIT_REVIEW_REQUIRED = 2
EXIT_MIGRATION_REQUIRED = 3
EXIT_ERROR = 4

VERDICT_CONTINUE = "continue"
VERDICT_ACCEPT_REQUIRED = "accept_required"
VERDICT_BLOCKED = "blocked"
VERDICT_INITIALIZED = "baseline_initialized"


@dataclass(frozen=True)
class GateDecision:
    allowed: bool
    verdict: str  # continue | accept_required | blocked
    overall: str
    reason: str

    def to_dict(self) -> Dict[str, Any]:
        return {
            "allowed": self.allowed,
            "verdict": self.verdict,
            "overall": self.overall,
            "reason": self.reason,
        }


class ContinuationGate:
    """Enforces: never continue on BREAKING; REVIEW needs explicit
    acceptance; SAFE/IDENTICAL continue."""

    def __init__(self, accept_review: bool = False):
        self.accept_review = accept_review

    def decide(self, report: ClassificationReport) -> GateDecision:
        overall = report.overall
        if overall in (IDENTICAL, SAFE):
            return GateDecision(
                True, VERDICT_CONTINUE, overall, "no guarded change"
            )
        if overall == REVIEW:
            if self.accept_review:
                return GateDecision(
                    True,
                    VERDICT_CONTINUE,
                    overall,
                    "REVIEW changes explicitly accepted via --accept-review",
                )
            return GateDecision(
                False,
                VERDICT_ACCEPT_REQUIRED,
                overall,
                "REVIEW changes require --accept-review",
            )
        return GateDecision(
            False,
            VERDICT_BLOCKED,
            overall,
            "BREAKING change: automatic continuation forbidden; "
            "state is MIGRATION_REQUIRED",
        )


def verify_runtime(
    runtime_dir,
    state_dir,
    *,
    accept_review: bool = False,
    seed: Optional[int] = None,
) -> Dict[str, Any]:
    """Run one verification pass.  Returns a result dict with
    ``exit_code`` and full detail (also persisted to the state file)."""
    runtime_dir = Path(runtime_dir)
    state_dir = Path(state_dir)
    recorded = run_metadata(
        runtime_dir=runtime_dir,
        seed=seed,
        config={"tool": "verify", "accept_review": accept_review},
    )

    try:
        manifest = collect_manifest(runtime_dir)
    except Exception as e:  # unparseable runtime config, etc.
        return {
            "exit_code": EXIT_ERROR,
            "verdict": "error",
            "error": f"manifest collection failed: {e}",
            "recorded": recorded,
        }
    identity = runtime_identity_hash(manifest)

    try:
        state = load_state(state_dir)
    except Exception as e:
        return {
            "exit_code": EXIT_ERROR,
            "verdict": "error",
            "error": str(e),
            "recorded": recorded,
        }

    # --- first run: establish baseline ---------------------------------
    if state.get("baseline") is None:
        missing = missing_required_fields(manifest)
        if missing:
            return {
                "exit_code": EXIT_ERROR,
                "verdict": "error",
                "error": f"incomplete runtime, missing required fields: {missing}",
                "identity_hash": identity,
                "recorded": recorded,
            }
        state["baseline"] = {
            "manifest": manifest,
            "identity_hash": identity,
            "recorded": recorded,
        }
        state["status"] = STATUS_OK
        state["history"].append(
            {
                "event": "baseline_initialized",
                "identity_hash": identity,
                "recorded": recorded,
            }
        )
        save_state(state_dir, state)
        return {
            "exit_code": EXIT_OK,
            "verdict": VERDICT_INITIALIZED,
            "overall": IDENTICAL,
            "changes": [],
            "identity_hash": identity,
            "state_status": state["status"],
            "recorded": recorded,
        }

    baseline = state["baseline"]
    changes = diff_manifests(baseline["manifest"], manifest)
    report = classify_changes(changes)
    decision = ContinuationGate(accept_review=accept_review).decide(report)

    history_entry = {
        "event": "verify",
        "identity_hash": identity,
        "baseline_identity_hash": baseline["identity_hash"],
        "overall": report.overall,
        "decision": decision.to_dict(),
        "n_changes": len(changes),
        "recorded": recorded,
    }
    state["history"].append(history_entry)

    result: Dict[str, Any] = {
        "verdict": decision.verdict,
        "overall": report.overall,
        "changes": [e.to_dict() for e in report.entries],
        "identity_hash": identity,
        "baseline_identity_hash": baseline["identity_hash"],
        "decision": decision.to_dict(),
        "recorded": recorded,
    }

    if report.overall == BREAKING:
        state["status"] = STATUS_MIGRATION_REQUIRED
        mig_path = record_migration(
            state_dir,
            old_manifest=baseline["manifest"],
            new_manifest=manifest,
            diff=[c.to_dict() for c in changes],
            classification=report.to_dict(),
            recorded=recorded,
        )
        state["migration"] = {
            "path": str(mig_path),
            "recorded": recorded,
            "old_identity_hash": baseline["identity_hash"],
            "new_identity_hash": identity,
        }
        result["exit_code"] = EXIT_MIGRATION_REQUIRED
        result["migration_record"] = str(mig_path)
    elif report.overall == REVIEW and not accept_review:
        result["exit_code"] = EXIT_REVIEW_REQUIRED
        # Baseline stays pinned; nothing was accepted.
    else:
        # IDENTICAL / SAFE, or REVIEW explicitly accepted: advance the
        # baseline to the current manifest and clear MIGRATION_REQUIRED
        # (a reverted runtime diffs IDENTICAL against the pinned
        # baseline and lands here).
        state["baseline"] = {
            "manifest": manifest,
            "identity_hash": identity,
            "recorded": recorded,
        }
        state["status"] = STATUS_OK
        state["migration"] = None
        result["exit_code"] = EXIT_OK

    result["state_status"] = state["status"]
    save_state(state_dir, state)
    return result


def _print_human(result: Dict[str, Any]) -> None:
    print(f"verdict:                {result.get('verdict')}")
    if "error" in result:
        print(f"error:                  {result['error']}")
        return
    print(f"overall classification: {result.get('overall')}")
    print(f"identity_hash:          {result.get('identity_hash')}")
    if result.get("baseline_identity_hash"):
        print(f"baseline_identity_hash: {result['baseline_identity_hash']}")
    print(f"state:                  {result.get('state_status')}")
    changes = result.get("changes") or []
    if changes:
        print("changes:")
        for c in changes:
            print(
                f"  [{c['classification']:8s}] {c['component']}: "
                f"{c['old']!r} -> {c['new']!r}  ({c['reason']})"
            )
    else:
        print("changes:                none")
    if result.get("migration_record"):
        print(f"migration record:       {result['migration_record']}")
    print(f"decision:               {result.get('decision', {}).get('reason', '')}")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="fi3-verify",
        description="Verify runtime identity against the stored baseline.",
    )
    p.add_argument("--runtime-dir", required=True)
    p.add_argument(
        "--state-dir",
        required=True,
        help="directory holding state.json (baseline manifest + status)",
    )
    p.add_argument(
        "--accept-review",
        action="store_true",
        help="explicitly accept REVIEW-classified changes",
    )
    p.add_argument("--json", action="store_true")
    p.add_argument("--seed", type=int, default=None)
    return p


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    result = verify_runtime(
        args.runtime_dir,
        args.state_dir,
        accept_review=args.accept_review,
        seed=args.seed,
    )
    if args.json:
        print(json.dumps(result, indent=2, sort_keys=True, ensure_ascii=False))
    else:
        _print_human(result)
    return int(result["exit_code"])


if __name__ == "__main__":
    sys.exit(main())
