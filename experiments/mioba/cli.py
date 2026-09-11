"""MIOBA CLI — `python -m experiments.mioba.cli` (wrapper: scripts/mioba)."""
from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone
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
            resume=args.resume)
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
                 "--execution-batch",
                 str(config.get("worker", {}).get(
                     "execution_batch",
                     config.get("worker", {}).get("batch_size", "auto")))],
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
    """Startup benchmark on one device: candidate batch sizes, sim-s/wall-s,
    VRAM, failure/OOM, selected batch. Written as JSON together with the
    runtime info (GPU, driver, CUDA, torch, git commit) when --out is
    given."""
    from .development.phenotype import develop
    from .fba.registry import get_backend
    from .fba.runtime_info import collect_runtime_info
    from .genome.schema import fba0_genome
    from .workers.bench import choose_batch, startup_benchmark
    from .workers.gpu_info import gpu_identity
    kw = {"synthetic": not args.data_dir, "data_dir": args.data_dir,
          "synthetic_neurons": args.synthetic_neurons}
    if args.synthetic_edges:
        kw["synthetic_edges"] = args.synthetic_edges
    backend = get_backend(args.backend, **kw)
    candidates = tuple(int(c) for c in args.candidates.split(","))
    rows = startup_benchmark(backend, develop(fba0_genome()),
                             device=args.device, candidates=candidates,
                             duration_ms=args.duration_ms)
    selected = choose_batch(rows, vram_headroom=args.vram_headroom)
    report = {"device": args.device, "backend": args.backend,
              "dataset": backend.dataset_identity(),
              "duration_ms": args.duration_ms,
              "vram_headroom": args.vram_headroom,
              "rows": [r.to_dict() for r in rows],
              "selected_batch": selected,
              "gpu_identity": gpu_identity(args.device),
              "runtime": collect_runtime_info(backend=args.backend,
                                              device=args.device)}
    for r in rows:
        print(r.to_dict())
    print(f"selected_batch={selected}")
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(json.dumps(report, indent=2, default=str))
        print(f"wrote {args.out}")
    return 0


def cmd_profile(args) -> int:
    """M1 §2: per-phase breakdown of one evaluation and the concurrent
    slot sweep, on the config's own conditions.

    Reports successful evaluations/minute per slot count — the criterion
    §2.2 asks for — not per-job latency.
    """
    from .fba.runtime_info import collect_runtime_info
    from .genome.hashing import config_hash, scientific_config_hash
    from .perf.evalbench import (activity_break_even, founder_population,
                                 slot_sweep)
    from .workers.gpu_info import gpu_identity
    config = load_config(getattr(args, "config", None))
    genomes = founder_population(args.evaluations,
                                 base_seed=int(config.get("evolution", {})
                                               .get("mutation_seed", 0)))
    slots = tuple(int(s) for s in str(args.slots).split(","))
    report = slot_sweep(genomes, config, args.device, slot_candidates=slots,
                        execution_batch=args.execution_batch,
                        duration_ms=args.duration_ms,
                        replicates=args.replicates, data_dir=args.data_dir)
    report["config_hash"] = config_hash(config)
    report["scientific_config_hash"] = scientific_config_hash(config)
    report["gpu_identity"] = gpu_identity(args.device)
    report["runtime"] = collect_runtime_info(device=args.device)

    print(f"device={args.device} evaluations={args.evaluations} "
          f"execution_batch={args.execution_batch}")
    for row in report["rows"]:
        print(f"  slots={row['slots']} ok={row['succeeded']}/"
              f"{row['evaluations']} oom={row['oom']} "
              f"wall={row['wall_s']:.1f}s "
              f"evals/min={row['successful_evaluations_per_minute']} "
              f"median_latency={row['median_latency_s']}s")
        for name, ph in sorted(row["timing"]["phases"].items(),
                               key=lambda kv: -kv[1]["seconds"]):
            print(f"      {name:22s} {ph['seconds']:8.3f}s "
                  f"{ph['pct_of_wall']:5}%  x{ph['calls']}")
    print(f"selected_slots={report['selected_slots']} "
          f"({report['selected_evaluations_per_minute']} evals/min)")
    if args.activity_sweep:
        be = activity_break_even(config, args.device, data_dir=args.data_dir)
        report["activity_break_even"] = be
        print("activity sweep (event vs dense propagation):")
        for r in be["rows"]:
            print(f"  stim={r['stim_rate_hz']:>7} Hz  "
                  f"rate={r['mean_rate_hz']:8.3f} Hz  "
                  f"edges/step={r['active_edges_per_step']:>12}  "
                  f"ratio={r['active_edge_ratio']:<10}  "
                  f"event={r['event_wall_s']}s dense={r['dense_wall_s']}s  "
                  f"{'event' if r['event_faster'] else 'DENSE'} wins")
        print(f"  break_even_active_edge_ratio="
              f"{be['break_even_active_edge_ratio']} ({be['note']})")
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(json.dumps(report, indent=2, default=str))
        print(f"wrote {args.out}")
    return 0


def cmd_rank_check(args) -> int:
    """M1 §2.3: does a cheaper evaluator rank individuals like the gold
    one? Accepts only at Spearman rho >= 0.85 and top-8 overlap >= 6/8.
    """
    from .evolution.population import fitness_placeholder
    from .perf.evalbench import founder_population
    from .perf.rank_agreement import compare_evaluators
    config = load_config(getattr(args, "config", None))
    ev = config.get("evaluation", {})
    target = float(ev.get("target_rate_hz", 5.0))
    genomes = founder_population(args.population,
                                 base_seed=int(config.get("evolution", {})
                                               .get("mutation_seed", 0)))
    gold = {"duration_ms": args.gold_duration_ms or ev.get("duration_ms", 500),
            "replicates": args.gold_replicates or ev.get("replicates", 8)}
    cheap = {"duration_ms": args.cheap_duration_ms,
             "replicates": args.cheap_replicates}
    report = compare_evaluators(
        genomes, config, args.device, gold, cheap,
        score=lambda s: fitness_placeholder(s, target) or float("-inf"),
        execution_batch=args.execution_batch, data_dir=args.data_dir,
        k=args.top_k, min_rho=args.min_rho, min_overlap=args.min_overlap)
    print(f"gold  {gold}  {report['gold_wall_s']}s")
    print(f"cheap {cheap}  {report['cheap_wall_s']}s  "
          f"speedup x{report['speedup']}")
    print(f"spearman_rho={report['spearman_rho']} "
          f"top{report['top_k']['k']}_overlap="
          f"{report['top_k']['overlap']}/{report['top_k']['k']}")
    print("ACCEPTED" if report["accepted"]
          else "REJECTED: " + "; ".join(report["rejected_because"]))
    if args.out:
        Path(args.out).parent.mkdir(parents=True, exist_ok=True)
        Path(args.out).write_text(json.dumps(report, indent=2, default=str))
        print(f"wrote {args.out}")
    return 0 if report["accepted"] else 6


def cmd_env_info(args) -> int:
    from .fba.runtime_info import collect_runtime_info
    print(json.dumps(collect_runtime_info(device=args.device), indent=2))
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
                          strict=args.strict,
                          allow_device_drift=args.allow_device_drift,
                          execution_batch=args.execution_batch)
        for w in plan.warnings:
            print(f"warning: {w}", file=sys.stderr)
        result = run_plan(plan)
    except ReplayUnavailable as exc:
        print(f"ReplayUnavailable: {exc}", file=sys.stderr)
        return 4
    except ReplayConfigMismatch as exc:
        print(f"ReplayConfigMismatch (strict): {exc}", file=sys.stderr)
        return 5
    payload = json.dumps(result, indent=2, default=str)
    # Every replay of one evaluation is kept: strict / --allow-device-drift /
    # --execution-batch runs of the same evaluation_id each record different
    # device_warnings, and overwriting one file loses them. The archived copy
    # is timestamped; <evaluation_id>.json stays as the "latest" pointer.
    replays = exp_dir / "replays"
    replays.mkdir(parents=True, exist_ok=True)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    archived = replays / f"{args.evaluation_id}.{stamp}.json"
    archived.write_text(payload)
    out = Path(args.out) if getattr(args, "out", None) else (
        replays / f"{args.evaluation_id}.json")
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(payload)
    print(json.dumps(result["diff"], indent=2))
    print(f"wrote {out}")
    print(f"archived {archived}")
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
    p.add_argument("--resume", default=None, metavar="EXPERIMENT_ID",
                   help="resume; scientific config sections must be "
                        "unchanged (a scientific change is a new experiment)")
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
                   help="fail instead of warning on config/git/backend drift "
                        "and on GPU model / compute-capability drift")
    p.add_argument("--allow-device-drift", action="store_true",
                   help="with --strict: permit a different GPU model/CC as an "
                        "explicit cross-GPU parity check (recorded)")
    p.add_argument("--execution-batch", type=int, default=None,
                   help="lanes per chunk for the replay (operational; "
                        "default: recorded execution batch)")
    p.add_argument("--out", default=None,
                   help="write the report here instead of "
                        "<run>/replays/<evaluation_id>.json (a timestamped "
                        "copy is archived under <run>/replays/ either way)")
    p.set_defaults(fn=cmd_replay)

    p = sub.add_parser("bench")
    p.add_argument("--device", default="cpu")
    p.add_argument("--backend", default="mock")
    p.add_argument("--duration-ms", type=float, default=200)
    p.add_argument("--candidates", default="1,2,4,8,16,32")
    p.add_argument("--synthetic-neurons", type=int, default=2000)
    p.add_argument("--synthetic-edges", type=int, default=None,
                   help="explicit synthetic edge count (default: "
                        "connectivity * N^2)")
    p.add_argument("--vram-headroom", type=float, default=0.85,
                   help="max reserved/total VRAM for an eligible batch")
    p.add_argument("--data-dir", default=None,
                   help="real FBA dataset dir (default: synthetic)")
    p.add_argument("--out", default=None, help="write JSON report here")
    p.set_defaults(fn=cmd_bench)

    p = sub.add_parser("profile", help="M1 evaluation phase breakdown + "
                                       "concurrent slot sweep")
    p.add_argument("--device", default="cpu")
    p.add_argument("--evaluations", type=int, default=4,
                   help="genomes evaluated per slot count")
    p.add_argument("--slots", default="1",
                   help="comma-separated concurrent evaluation slots, "
                        "e.g. 1,2,3")
    p.add_argument("--execution-batch", type=int, default=1,
                   help="GPU lanes per chunk (operational)")
    p.add_argument("--duration-ms", type=float, default=None,
                   help="override evaluation.duration_ms")
    p.add_argument("--replicates", type=int, default=None,
                   help="override evaluation.replicates")
    p.add_argument("--data-dir", default=None)
    p.add_argument("--activity-sweep", action="store_true",
                   help="also measure where dense propagation overtakes "
                        "the event path (sets fba.dense_above)")
    p.add_argument("--out", default=None, help="write JSON report here")
    p.set_defaults(fn=cmd_profile)

    p = sub.add_parser("rank-check",
                       help="M1 cheap-vs-gold evaluator rank agreement")
    p.add_argument("--device", default="cpu")
    p.add_argument("--population", type=int, default=16)
    p.add_argument("--gold-duration-ms", type=float, default=None)
    p.add_argument("--gold-replicates", type=int, default=None)
    p.add_argument("--cheap-duration-ms", type=float, default=250.0)
    p.add_argument("--cheap-replicates", type=int, default=2)
    p.add_argument("--execution-batch", type=int, default=1)
    p.add_argument("--top-k", type=int, default=8)
    p.add_argument("--min-rho", type=float, default=0.85)
    p.add_argument("--min-overlap", type=int, default=6)
    p.add_argument("--data-dir", default=None)
    p.add_argument("--out", default=None)
    p.set_defaults(fn=cmd_rank_check)

    p = sub.add_parser("worker", add_help=False)
    p.add_argument("worker_args", nargs=argparse.REMAINDER)
    p.set_defaults(fn=cmd_worker)

    p = sub.add_parser("env-info")
    p.add_argument("--device", default=None,
                   help="describe this CUDA device (e.g. cuda:1)")
    p.set_defaults(fn=cmd_env_info)
    return ap


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    return args.fn(args)


if __name__ == "__main__":
    sys.exit(main())
