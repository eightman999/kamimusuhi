"""Subprocess worker for the SIGKILL crash harness.

Usage (process mode):
    python -m experiments.fi1.fault.worker --db DB --turns turns.jsonl \
        [--crash-point POINT --marker FILE] [--seed N] [--synchronous L]

    Receives each envelope in the JSONL through a real TurnEngine backed by
    a FileSink at ``<db>.sink.jsonl`` (the durable "external world"). When
    the injector reaches ``--crash-point`` it durably writes ``--marker``
    and blocks; the parent then sends SIGKILL. All committed WAL state
    survives.

Usage (recovery mode):
    python -m experiments.fi1.fault.worker --db DB --recover \
        [--report out.json] [--orphan-policy abort|resume]

    Opens the store in a fresh process, replays the WAL, and writes the
    recovery report + metrics to ``--report`` (or stdout).
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from ..runtime.engine import TurnEngine
from ..runtime.mocks import FileSink
from .injector import FaultInjector, marker_and_block


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="experiments.fi1.fault.worker")
    p.add_argument("--db", required=True)
    p.add_argument("--turns", help="JSONL of envelopes to receive")
    p.add_argument("--crash-point")
    p.add_argument("--marker")
    p.add_argument("--seed", type=int, default=0)
    p.add_argument("--synchronous", default="FULL")
    p.add_argument("--recover", action="store_true")
    p.add_argument("--orphan-policy", default="abort",
                   choices=["abort", "resume"])
    p.add_argument("--report")
    return p


def main(argv=None) -> int:
    args = _build_parser().parse_args(argv)
    sink_path = Path(args.db).with_suffix(".sink.jsonl")
    sink = FileSink(sink_path)

    injector = None
    if args.crash_point:
        injector = FaultInjector({
            args.crash_point: marker_and_block(args.marker),
        })

    engine = TurnEngine(args.db, seed=args.seed, sink=sink,
                        injector=injector,
                        synchronous=args.synchronous,
                        orphan_policy=args.orphan_policy,
                        run_role="recover" if args.recover else "worker")

    if args.recover:
        report = engine.recover()
        out = {"recovery": report, "metrics": engine.metrics(),
               "sink_invocations": sink.invocation_count,
               "sink_applied": sink.applied_count}
        text = json.dumps(out, indent=2, sort_keys=True)
        if args.report:
            Path(args.report).write_text(text)
        else:
            print(text)
        engine.close()
        sink.close()
        return 0

    receipts = []
    with open(args.turns) as f:
        for line in f:
            if line.strip():
                receipts.append(engine.receive(line.strip()))
    out = {"receipts": receipts, "metrics": engine.metrics()}
    if args.report:
        Path(args.report).write_text(json.dumps(out, indent=2,
                                                sort_keys=True))
    else:
        print(json.dumps(out))
    engine.close()
    sink.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
