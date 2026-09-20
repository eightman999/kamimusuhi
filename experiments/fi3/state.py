"""Persistent guard state — previous manifest + continuation status.

Stored as a small JSON file (``<state-dir>/state.json``) so restart
comparison is real: ``verify`` always compares the freshly collected
manifest against the *stored baseline*, not against anything in memory.

State transitions:

    (no baseline)            -> baseline initialized, status OK
    IDENTICAL/SAFE           -> baseline updated to current, status OK
    REVIEW + --accept-review -> baseline updated to current, status OK
    REVIEW (no flag)         -> baseline kept, status unchanged, deny
    BREAKING                 -> baseline kept, status MIGRATION_REQUIRED,
                                migration record written

After MIGRATION_REQUIRED the baseline stays pinned; a runtime reverted
to baseline diffs IDENTICAL and clears the status back to OK.

Writes are atomic (tmp + rename) so a crash cannot truncate the state.
"""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Any, Dict, List, Optional, Union

STATE_VERSION = 1
STATE_FILENAME = "state.json"
MIGRATION_FILENAME = "migration_required.json"

STATUS_OK = "OK"
STATUS_MIGRATION_REQUIRED = "MIGRATION_REQUIRED"

STATUSES = (STATUS_OK, STATUS_MIGRATION_REQUIRED)


class StateError(Exception):
    pass


def default_state() -> Dict[str, Any]:
    return {
        "state_version": STATE_VERSION,
        "status": STATUS_OK,
        "baseline": None,  # {"manifest":..., "identity_hash":..., "recorded":...}
        "migration": None,  # written on BREAKING
        "history": [],  # raw log of every verify event
    }


def state_path(state_dir: Union[str, Path]) -> Path:
    return Path(state_dir) / STATE_FILENAME


def migration_path(state_dir: Union[str, Path]) -> Path:
    return Path(state_dir) / MIGRATION_FILENAME


def load_state(state_dir: Union[str, Path]) -> Dict[str, Any]:
    """Load state, or a fresh default if none exists yet."""
    path = state_path(state_dir)
    if not path.exists():
        return default_state()
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as e:
        raise StateError(f"corrupt state file {path}: {e}") from e
    if not isinstance(raw, dict) or raw.get("state_version") != STATE_VERSION:
        raise StateError(f"unsupported state file {path}")
    state = default_state()
    state.update(raw)
    return state


def save_state(state_dir: Union[str, Path], state: Dict[str, Any]) -> Path:
    """Atomically write state.json."""
    state_dir = Path(state_dir)
    state_dir.mkdir(parents=True, exist_ok=True)
    path = state_path(state_dir)
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(
        json.dumps(state, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    os.replace(tmp, path)
    return path


def record_migration(
    state_dir: Union[str, Path],
    *,
    old_manifest: Dict[str, Any],
    new_manifest: Dict[str, Any],
    diff: List[Dict[str, Any]],
    classification: Dict[str, Any],
    recorded: Dict[str, Any],
) -> Path:
    """Write the Fi4 migration hook payload: old/new manifests + diff +
    classification + provenance.  Returns the file path."""
    state_dir = Path(state_dir)
    state_dir.mkdir(parents=True, exist_ok=True)
    payload = {
        "record_type": "fi3-migration-required",
        "record_version": 1,
        "old_manifest": old_manifest,
        "new_manifest": new_manifest,
        "diff": diff,
        "classification": classification,
        "recorded": recorded,
    }
    path = migration_path(state_dir)
    tmp = path.with_suffix(".json.tmp")
    tmp.write_text(
        json.dumps(payload, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        encoding="utf-8",
    )
    os.replace(tmp, path)
    return path
