"""Component → classification policy for runtime identity changes.

Every manifest component field maps to exactly one rule producing
SAFE / REVIEW / BREAKING:

* **SAFE** — operational-only config (log dir, UI theme).  Continuation
  allowed.
* **REVIEW** — same model family but behavior-relevant drift: sampling
  parameters, tool schema, minor/patch runtime or schema version bumps,
  symlink retargets, raw prompt bytes that canonicalize identically.
  Continuation requires explicit ``--accept-review``.
* **BREAKING** — identity-level change: different model/tokenizer
  (name, hash or revision), semantically changed prompts (canonical
  hash), adapter change/removal, major version bumps, manifest format
  changes, deleted/appearing components, and — conservatively — any
  component with no policy rule.  Automatic continuation is forbidden;
  state moves to ``MIGRATION_REQUIRED``.

Rules may be a plain classification or ``fn(old, new) -> (class, reason)``
for value-aware decisions (semver splits major→BREAKING / minor→REVIEW).
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Callable, Dict, Iterable, List, Optional, Tuple, Union

from ..identity.diff import ADDED, MODIFIED, REMOVED, Change
from ..identity.manifest import COMPONENT_SCHEMA
from ..identity.canonical import flatten_components

IDENTICAL = "IDENTICAL"
SAFE = "SAFE"
REVIEW = "REVIEW"
BREAKING = "BREAKING"

# Overall verdicts only ever reach these; IDENTICAL means "no changes".
CLASSIFICATIONS = (IDENTICAL, SAFE, REVIEW, BREAKING)

_SEVERITY = {IDENTICAL: -1, SAFE: 0, REVIEW: 1, BREAKING: 2}
_UNKNOWN_COMPONENT_CLASS = BREAKING  # conservative default

RuleResult = Tuple[str, str]
Rule = Union[str, Callable[[Any, Any], RuleResult]]


class PolicyGap(Exception):
    """A manifest field lacks a classification rule (or vice versa)."""


def _semver_split(component: str) -> Callable[[Any, Any], RuleResult]:
    """major bump -> BREAKING; minor/patch -> REVIEW; junk -> BREAKING."""

    def rule(old: Any, new: Any) -> RuleResult:
        try:
            old_major = int(str(old).split(".")[0])
            new_major = int(str(new).split(".")[0])
        except (TypeError, ValueError):
            return (
                BREAKING,
                f"{component}: unparseable version {old!r} -> {new!r}; "
                "conservative BREAKING",
            )
        if old_major != new_major:
            return (
                BREAKING,
                f"{component}: major version change {old!r} -> {new!r}",
            )
        return (REVIEW, f"{component}: minor/patch change {old!r} -> {new!r}")

    return rule


def _fixed(cls: str, reason: str) -> Callable[[Any, Any], RuleResult]:
    return lambda old, new: (cls, reason)


# The policy table.  Keys are dotted component paths (or the
# "manifest_version" pseudo-field).  Completeness against the manifest
# schema is enforced by assert_policy_completeness() and a unit test.
POLICY_TABLE: Dict[str, Rule] = {
    # --- format -----------------------------------------------------
    "manifest_version": _fixed(
        BREAKING, "manifest format changed; cross-version comparison untrusted"
    ),
    # --- model identity ---------------------------------------------
    "model.name": _fixed(BREAKING, "different model"),
    "model.hash": _fixed(BREAKING, "model weights content changed"),
    "model.revision": _fixed(BREAKING, "model revision changed (silent-pull vector)"),
    "model.link": _fixed(REVIEW, "model symlink retargeted"),
    # --- tokenizer ----------------------------------------------------
    "tokenizer.name": _fixed(BREAKING, "different tokenizer"),
    "tokenizer.hash": _fixed(BREAKING, "tokenizer content changed"),
    "tokenizer.link": _fixed(REVIEW, "tokenizer symlink retargeted"),
    # --- prompts ------------------------------------------------------
    # Raw-byte change alone is never SAFE (conservative Fi3 stance); a
    # semantic change escalates to BREAKING via canonical_hash.
    "system_prompt.hash": _fixed(
        REVIEW, "system prompt bytes changed (canonical form may be equal)"
    ),
    "system_prompt.canonical_hash": _fixed(
        BREAKING, "system prompt content changed (canonical form differs)"
    ),
    "prompt_template.hash": _fixed(
        REVIEW, "prompt template bytes changed (canonical form may be equal)"
    ),
    "prompt_template.canonical_hash": _fixed(
        BREAKING, "prompt template content changed (canonical form differs)"
    ),
    # --- adapter --------------------------------------------------------
    "adapter.present": _fixed(BREAKING, "adapter added or removed"),
    "adapter.hash": _fixed(BREAKING, "adapter weights content changed"),
    "adapter.link": _fixed(REVIEW, "adapter symlink retargeted"),
    # --- sampling identity ----------------------------------------------
    # "same model" is not "same runtime": any sampling drift is at least
    # REVIEW, never silently continued.
    "sampling.temperature": _fixed(REVIEW, "sampling temperature changed"),
    "sampling.top_p": _fixed(REVIEW, "sampling top_p changed"),
    "sampling.top_k": _fixed(REVIEW, "sampling top_k changed"),
    "sampling.seed": _fixed(REVIEW, "sampling seed changed"),
    "sampling.repetition_penalty": _fixed(
        REVIEW, "sampling repetition_penalty changed"
    ),
    "sampling.context_length": _fixed(REVIEW, "sampling context_length changed"),
    # --- versions ---------------------------------------------------------
    "versions.runtime": _semver_split("versions.runtime"),
    "versions.memory_schema": _semver_split("versions.memory_schema"),
    "versions.tool_schema": _fixed(REVIEW, "tool schema version changed"),
    "tool_schema.hash": _fixed(REVIEW, "tool schema definition changed"),
    "dependency_lock.hash": _fixed(REVIEW, "dependency lockfile changed"),
    # --- operational (SAFE examples) ---------------------------------------
    "operational.log_dir": _fixed(SAFE, "logging path changed"),
    "operational.ui_theme": _fixed(SAFE, "UI-only config changed"),
}


def expected_fields() -> List[str]:
    """Every field a manifest can carry (schema fields + pseudo-field)."""
    return sorted(flatten_components(COMPONENT_SCHEMA)) + ["manifest_version"]


def assert_policy_completeness(
    table: Optional[Dict[str, Rule]] = None,
    fields: Optional[Iterable[str]] = None,
) -> None:
    """Fail unless every manifest field has a rule and vice versa.

    Defense against an unclassified component: adding a field to
    ``COMPONENT_SCHEMA`` without a policy, or a stale policy key, raises
    :class:`PolicyGap`.
    """
    table = POLICY_TABLE if table is None else table
    fields = set(expected_fields() if fields is None else fields)
    missing = sorted(fields - set(table))
    stale = sorted(set(table) - fields)
    problems = []
    if missing:
        problems.append(f"fields without policy: {missing}")
    if stale:
        problems.append(f"policy keys with no manifest field: {stale}")
    if problems:
        raise PolicyGap("; ".join(problems))


@dataclass(frozen=True)
class ClassifiedChange:
    change: Change
    classification: str
    reason: str

    def to_dict(self) -> Dict[str, Any]:
        d = self.change.to_dict()
        d["classification"] = self.classification
        d["reason"] = self.reason
        return d


@dataclass(frozen=True)
class ClassificationReport:
    overall: str  # IDENTICAL | SAFE | REVIEW | BREAKING
    entries: Tuple[ClassifiedChange, ...]

    def to_dict(self) -> Dict[str, Any]:
        return {
            "overall": self.overall,
            "changes": [e.to_dict() for e in self.entries],
        }


def classify_change(change: Change) -> ClassifiedChange:
    rule = POLICY_TABLE.get(change.component)
    if rule is None:
        # Conservative: unclassified drift is never silently continued.
        return ClassifiedChange(
            change,
            _UNKNOWN_COMPONENT_CLASS,
            f"{change.component}: no policy rule; conservative BREAKING",
        )
    if isinstance(rule, str):
        cls, reason = rule, f"{change.component}: {change.kind}"
    else:
        cls, reason = rule(change.old, change.new)
    if change.kind in (ADDED, REMOVED) and _SEVERITY[cls] < _SEVERITY[REVIEW]:
        # A SAFE field appearing/disappearing entirely is still odd —
        # floor at REVIEW for structural adds/removes.
        cls, reason = (
            REVIEW,
            f"{change.component}: component {change.kind}; "
            "structural drift is at least REVIEW",
        )
    return ClassifiedChange(change, cls, reason)


def classify_changes(changes: Iterable[Change]) -> ClassificationReport:
    entries = tuple(classify_change(c) for c in changes)
    if not entries:
        return ClassificationReport(IDENTICAL, entries)
    overall = max(entries, key=lambda e: _SEVERITY[e.classification]).classification
    return ClassificationReport(overall, entries)


def severity_rank(classification: str) -> int:
    return _SEVERITY[classification]
