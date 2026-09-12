"""Reproducibility metadata recorded on every inspect/verify run.

Captures git commit, branch, dirty flag, Python version, UTC timestamp,
plus the caller-supplied seed/config.  Everything is best-effort: a
missing git binary or non-git tree yields ``None`` fields rather than an
error, but the fields are always present in the record.
"""

from __future__ import annotations

import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Dict, Optional, Union


def _git(args, cwd) -> Optional[str]:
    try:
        out = subprocess.run(
            ["git", *args],
            cwd=str(cwd),
            capture_output=True,
            text=True,
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if out.returncode != 0:
        return None
    return out.stdout.strip()


def _find_repo_root(start: Path) -> Optional[Path]:
    for cand in (start, *start.parents):
        if (cand / ".git").exists():
            return cand
    return None


def run_metadata(
    *,
    runtime_dir: Union[str, Path, None] = None,
    seed: Optional[int] = None,
    config: Optional[Dict[str, Any]] = None,
) -> Dict[str, Any]:
    """Collect the reproducibility record for one run."""
    repo = _find_repo_root(Path(__file__).resolve())
    dirty = None
    if repo is not None:
        status = _git(["status", "--porcelain"], repo)
        dirty = None if status is None else bool(status.strip())
    return {
        "git_commit": _git(["rev-parse", "HEAD"], repo) if repo else None,
        "git_branch": (
            _git(["rev-parse", "--abbrev-ref", "HEAD"], repo) if repo else None
        ),
        "git_dirty": dirty,
        "python_version": sys.version.split()[0],
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
        "seed": seed,
        "config": config or {},
        "runtime_dir": str(runtime_dir) if runtime_dir is not None else None,
    }
