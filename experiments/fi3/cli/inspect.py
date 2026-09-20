"""``inspect`` CLI — collect a runtime manifest and print its identity.

Usage::

    python -m experiments.fi3.cli.inspect --runtime-dir /path/to/runtime
    python -m experiments.fi3.cli.inspect --runtime-dir R --json

Prints the manifest, the ``runtime_identity_hash``, any missing required
fields, and the reproducibility record (git commit/branch, Python
version, timestamp, seed).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import List, Optional

from ..identity.manifest import (
    collect_manifest,
    missing_required_fields,
    runtime_identity_hash,
)
from ..repro import run_metadata


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="fi3-inspect",
        description="Collect a Fi3 runtime manifest and print its identity hash.",
    )
    p.add_argument(
        "--runtime-dir",
        required=True,
        help="path to the runtime directory to inspect",
    )
    p.add_argument(
        "--json", action="store_true", help="emit the full record as JSON"
    )
    p.add_argument(
        "--seed",
        type=int,
        default=None,
        help="seed recorded in the reproducibility metadata",
    )
    return p


def inspect_runtime(
    runtime_dir, *, seed: Optional[int] = None
) -> dict:
    """Collect manifest + identity + provenance as a plain dict."""
    manifest = collect_manifest(Path(runtime_dir))
    return {
        "runtime_dir": str(runtime_dir),
        "runtime_identity_hash": runtime_identity_hash(manifest),
        "manifest": manifest,
        "missing_required_fields": missing_required_fields(manifest),
        "recorded": run_metadata(
            runtime_dir=runtime_dir, seed=seed, config={"tool": "inspect"}
        ),
    }


def main(argv: Optional[List[str]] = None) -> int:
    args = build_parser().parse_args(argv)
    record = inspect_runtime(args.runtime_dir, seed=args.seed)
    if args.json:
        print(json.dumps(record, indent=2, sort_keys=True, ensure_ascii=False))
    else:
        print(f"runtime_dir:            {record['runtime_dir']}")
        print(f"runtime_identity_hash:  {record['runtime_identity_hash']}")
        print(f"manifest_version:       {record['manifest']['manifest_version']}")
        missing = record["missing_required_fields"]
        print(f"missing_required:       {missing if missing else 'none'}")
        rec = record["recorded"]
        print(f"git:                    {rec['git_branch']}@{rec['git_commit']}"
              f"{' (dirty)' if rec['git_dirty'] else ''}")
        print(f"python:                 {rec['python_version']}")
        print(f"timestamp_utc:          {rec['timestamp_utc']}")
        print("components:")
        for group, fields in sorted(record["manifest"]["components"].items()):
            print(f"  {group}:")
            for k, v in sorted(fields.items()):
                shown = v
                if isinstance(v, str) and len(v) > 64:
                    shown = v[:61] + "..."
                print(f"    {k}: {shown}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
