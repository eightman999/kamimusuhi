"""MIOBA CLI — `python -m experiments.mioba.cli` (wrapper: scripts/mioba)."""
from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from pathlib import Path

import yaml

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_RUNS = REPO_ROOT / "experiments" / "mioba" / ".runs"


def load_config(path: str | None) -> dict:
    if path is None:
        path = str(REPO_ROOT / "experiments" / "mioba" / "configs"
                   / "default.yaml")
    with open(path) as fh:
        return yaml.safe_load(fh)


def _runs_dir(args, config) -> Path:
    rd = getattr(args, "runs_dir", None) or \
        (config or {}).get("experiment", {}).get("runs_dir") or DEFAULT_RUNS
    p = Path(rd)
    return p if p.is_absolute() else REPO_ROOT / p


def _find_experiment(runs_dir: Path, experiment_id: str | None) -> Path:
    if experiment_id:
        d = runs_dir / experiment_id
        if not d.is_dir():
            raise SystemExit(f"no experiment {experiment_id} under {runs_dir}")
        return d
    candidates = [d for d in runs_dir.iterdir()
                  if (d / "coordinator.json").is_file()]
    if not candidates:
        raise SystemExit(f"no live experiment under {runs_dir}")
    return max(candidates, key=lambda d: d.stat().st_mtime)


def _coordinator(args, config):
    runs_dir = _runs_dir(args, config)
    exp_dir = _find_experiment(runs_dir, getattr(args, "experiment_id", None))
    meta = json.loads((exp_dir / "coordinator.json").read_text())
    base = f"http://{meta['host']}:{meta['port']}"
    return base, meta["token"], exp_dir


def _post(base, token, path, body=None):
    import httpx
    return httpx.post(base + path, json=body,
                      headers={"X-Mioba-Token": token}, timeout=30)


def _get(base, path, **params):
    import httpx
    return httpx.get(base + path, params=params or None, timeout=30)


# ----------------------------------------------------------------- commands
def cmd_start(args) -> int:
    config = load_config(args.config)
    runs_dir = _runs_dir(args, config)
    from .coordinator.app import create_app
    from .coordinator.service import (MiobaService,
                                      ScientificConfigMismatch)

    try:
        service = MiobaService(
            config, runs_dir, experiment_id=args.experiment_id,
            resume=args.resume,
            allow_scientific_change=args.allow_scientific_change)
    except ScientificConfigMismatch as exc:
        print(f"ScientificConfigMismatch: {exc}", file=sys.stderr)
        return 3
    (service.run_dir / "coordinator.json").write_text(json.dumps({
        "host": args.host, "port": args.port, "pid": os.getpid(),
        "token": service.token,
    }))
    service.start_background()

    procs = []
    if args.workers:
        py = sys.executable
        for spec in args.workers.split(","):
            parts = spec.split(":")
            device = ":".join(parts[:2]) if len(parts) > 1 else parts[0]
            backend = parts[-1] if len(parts) > 1 else "mock"
            procs.append(subprocess.Popen(
                [py, "-m", "experiments.mioba.workers.worker",
                 "--coordinator", f"http://{args.host}:{args.port}",
                 "--device", device, "--backend", backend,
                 "--batch", str(config.get("worker", {})
                                .get("batch_size", 1))],
                cwd=str(REPO_ROOT)))

    import uvicorn
    app = create_app(service)
    cfg = uvicorn.Config(app, host=args.host, port=args.port,
                         log_level="warning")
    server = uvicorn.Server(cfg)

    def shutdown_watcher():
        while not service.shutdown_requested:
            time.sleep(0.5)
        server.should_exit = True
        for p in procs:
            p.terminate()

    import threading
    threading.Thread(target=shutdown_watcher, daemon=True).start()
    server.run()
    service.stop_background()
    return 0


def cmd_status(args) -> int:
    config = load_config(getattr(args, "config", None))
    base, _tok, _ = _coordinator(args, config)
    data = _get(base, "/api/status").json()
    if args.json:
        print(json.dumps(data, indent=2))
    else:
        c = data["counters"]
        print(f"{data['experiment_id']}  {data['status']}  "
              f"up {data['uptime_s']}s  pop={data['population_size']}  "
              f"queued={c['queued']} running={c['running']} "
              f"unknown={c['unknown']} "
              f"eval+={c['evaluations_succeeded']} "
              f"eval-={c['evaluations_failed']}")
    return 0


def _control(args, path, body=None) -> int:
    config = load_config(getattr(args, "config", None))
    base, token, _ = _coordinator(args, config)
    r = _post(base, token, path, body)
    if r.status_code == 200:
        print(r.json() if args.json else f"{path}: ok")
        return 0
    print(f"{path}: HTTP {r.status_code} {r.text}", file=sys.stderr)
    return 1


def cmd_pause(args):
    return _control(args, "/api/control/pause")


def cmd_resume(args) -> int:
    config = load_config(getattr(args, "config", None))
    runs_dir = _runs_dir(args, config)
    exp_dir = _find_experiment(runs_dir, getattr(args, "experiment_id", None))
    meta_path = exp_dir / "coordinator.json"
    if meta_path.is_file():
        return _control(args, "/api/control/resume")
    print("coordinator is not running; use `mioba start --resume "
          f"{exp_dir.name}`", file=sys.stderr)
    return 1


def cmd_checkpoint(args):
    return _control(args, "/api/control/checkpoint")


def cmd_stop(args):
    return _control(args, "/api/control/stop",
                    {"timeout_s": args.timeout})


def cmd_bench(args) -> int:
    from .fba.registry import get_backend
    from .workers.bench import startup_benchmark
    backend = get_backend(args.backend)
    rows = startup_benchmark(backend, {}, device=args.device,
                             duration_ms=args.duration_ms)
    for r in rows:
        print(r.to_dict())
    return 0


def cmd_env_info(args) -> int:
    from .fba.runtime_info import collect_runtime_info
    print(json.dumps(collect_runtime_info(), indent=2))
    return 0


def cmd_replay(args) -> int:
    """Re-run one recorded evaluation under its recorded conditions
    (backend, dataset identity, seed, env, duration, batch, config
    snapshot). Fails with ReplayUnavailable rather than substituting a
    synthetic network for a real-FBA recording."""
    config = load_config(getattr(args, "config", None))
    runs_dir = _runs_dir(args, config)
    exp_dir = _find_experiment(runs_dir, getattr(args, "experiment_id", None))
    from .coordinator.replay import (ReplayConfigMismatch, ReplayUnavailable,
                                     build_plan, run_plan)
    from .fba.runtime_info import collect_runtime_info
    try:
        plan = build_plan(exp_dir, args.evaluation_id, device=args.device,
                          data_dir=args.data_dir, backend=args.backend,
                          current_config=config,
                          current_git_commit=collect_runtime_info().get(
                              "git_commit"),
                          strict=args.strict)
        for w in plan.warnings:
            print(f"warning: {w}", file=sys.stderr)
        result = run_plan(plan)
    except ReplayUnavailable as exc:
        print(f"ReplayUnavailable: {exc}", file=sys.stderr)
        return 4
    except ReplayConfigMismatch as exc:
        print(f"ReplayConfigMismatch (strict): {exc}", file=sys.stderr)
        return 5
    out = exp_dir / "replays" / f"{args.evaluation_id}.json"
    out.write_text(json.dumps(result, indent=2, default=str))
    print(json.dumps(result["diff"], indent=2))
    print(f"wrote {out}")
    return 0


def cmd_worker(args) -> int:
    from .workers import worker
    return worker.main(args.worker_args)


def build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(prog="mioba")
    ap.add_argument("--config", default=None)
    ap.add_argument("--runs-dir", default=None)
    ap.add_argument("--experiment-id", default=None)
    sub = ap.add_subparsers(dest="cmd", required=True)

    p = sub.add_parser("start")
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, default=8765)
    p.add_argument("--experiment-id", default=None)
    p.add_argument("--resume", default=None, metavar="EXPERIMENT_ID")
    p.add_argument("--allow-scientific-change", action="store_true",
                   help="resume even if result-affecting config sections "
                        "changed (recorded as scientific_config_mismatch)")
    p.add_argument("--workers", default=None,
                   help='e.g. "cuda:0:torch,cuda:1:torch"')
    p.set_defaults(fn=cmd_start)

    p = sub.add_parser("status")
    p.add_argument("--json", action="store_true")
    p.set_defaults(fn=cmd_status)

    for name, fn in (("pause", cmd_pause), ("resume", cmd_resume),
                     ("checkpoint", cmd_checkpoint)):
        p = sub.add_parser(name)
        p.add_argument("--json", action="store_true")
        p.set_defaults(fn=fn)

    p = sub.add_parser("stop")
    p.add_argument("--timeout", type=float, default=None)
    p.add_argument("--json", action="store_true")
    p.set_defaults(fn=cmd_stop)

    p = sub.add_parser("replay")
    p.add_argument("evaluation_id")
    p.add_argument("--backend", default=None,
                   help="override recorded backend (cross-backend parity "
                        "check; recorded as a warning)")
    p.add_argument("--device", default="cpu")
    p.add_argument("--data-dir", default=None,
                   help="location of the recorded real FBA dataset")
    p.add_argument("--strict", action="store_true",
                   help="fail instead of warning on config/git/backend/"
                        "device drift")
    p.set_defaults(fn=cmd_replay)

    p = sub.add_parser("bench")
    p.add_argument("--device", default="cpu")
    p.add_argument("--backend", default="mock")
    p.add_argument("--duration-ms", type=float, default=200)
    p.set_defaults(fn=cmd_bench)

    p = sub.add_parser("worker", add_help=False)
    p.add_argument("worker_args", nargs=argparse.REMAINDER)
    p.set_defaults(fn=cmd_worker)

    p = sub.add_parser("env-info")
    p.set_defaults(fn=cmd_env_info)
    return ap


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
