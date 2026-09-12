"""Reproducibility record: every run stores git commit, branch, config,
seed, Python version and timestamp in the ``runs`` table."""

from __future__ import annotations

import platform
import subprocess
from datetime import datetime, timezone
from typing import Any, Dict, Optional


def _git(args, cwd=None) -> Optional[str]:
    try:
        out = subprocess.run(["git"] + args, cwd=cwd, capture_output=True,
                             text=True, timeout=10)
        return out.stdout.strip() if out.returncode == 0 else None
    except Exception:
        return None


def run_metadata(seed: int = 0, config: Optional[Dict] = None,
                 cwd=None) -> Dict[str, Any]:
    return {
        "git_commit": _git(["rev-parse", "HEAD"], cwd),
        "branch": _git(["rev-parse", "--abbrev-ref", "HEAD"], cwd),
        "python_version": platform.python_version(),
        "timestamp_utc": datetime.now(timezone.utc).isoformat(),
        "seed": seed,
        "config": config or {},
    }
