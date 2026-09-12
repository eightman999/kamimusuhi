"""Structured per-component manifest diff.

Compares two manifests field-by-field (flattened dotted paths) plus the
``manifest_version`` pseudo-field, producing an ordered list of
:class:`Change` records.  The policy layer
(:mod:`experiments.fi3.policies.compatibility`) classifies each change.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, List, Optional

from .canonical import flatten_components

ADDED = "added"
REMOVED = "removed"
MODIFIED = "modified"


@dataclass(frozen=True)
class Change:
    """One field-level difference between two manifests."""

    component: str  # dotted path, e.g. "sampling.temperature"
    kind: str  # added | removed | modified
    old: Any
    new: Any

    def to_dict(self) -> Dict[str, Any]:
        return {
            "component": self.component,
            "kind": self.kind,
            "old": self.old,
            "new": self.new,
        }


def diff_manifests(
    old: Optional[Dict[str, Any]], new: Optional[Dict[str, Any]]
) -> List[Change]:
    """Field-level diff between two manifests (or None = absent).

    ``None`` manifests yield ``added``/``removed`` changes for every
    field — an appearing/disappearing runtime is a difference, never a
    silent pass.
    """
    old = old or {}
    new = new or {}
    changes: List[Change] = []

    if old.get("manifest_version") != new.get("manifest_version"):
        changes.append(
            Change(
                component="manifest_version",
                kind=MODIFIED,
                old=old.get("manifest_version"),
                new=new.get("manifest_version"),
            )
        )

    old_flat = flatten_components(old.get("components", {}))
    new_flat = flatten_components(new.get("components", {}))
    for key in sorted(set(old_flat) | set(new_flat)):
        in_old = key in old_flat
        in_new = key in new_flat
        if in_old and not in_new:
            changes.append(Change(key, REMOVED, old_flat[key], None))
        elif in_new and not in_old:
            changes.append(Change(key, ADDED, None, new_flat[key]))
        elif old_flat[key] != new_flat[key]:
            changes.append(Change(key, MODIFIED, old_flat[key], new_flat[key]))
    return changes
