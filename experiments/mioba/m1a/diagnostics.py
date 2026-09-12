"""M1A diagnostic harness — one timestep under the microscope.

Every subcommand runs a controlled comparison on the *real* backend
(``fba.torch_backend.TorchBackend``), never a reimplementation: the
recorded quantities are the actual tensors the simulator uses.

Per-step recording works by driving ``backend.step(1)`` in a loop and
reading backend state around it:

- ``delayed`` (the recurrent current arriving this step) is read from
  ``delay_buf[:, _delay_ptr]`` *before* the step — exactly what
  ``_one_step`` will consume.
- Poisson input events are recovered exactly: the per-lane generator
  state is saved before the step, then the draw is replayed after it.
  ``set_state(pre)`` + one ``rand`` leaves the generator in the same
  post-draw state the real step produced, so recording is side-effect
  free.
- output spikes are read from ``backend.spikes`` after the step.

Subcommands write JSON (machine-readable) into ``--out``; ``plots``
renders the required figures from those JSONs.

    python -m experiments.mioba.m1a.diagnostics --device cpu trace
    python -m experiments.mioba.m1a.diagnostics micro
    python -m experiments.mioba.m1a.diagnostics --neurons 20000 ablation
    python -m experiments.mioba.m1a.diagnostics --neurons 20000 sweep
    python -m experiments.mioba.m1a.diagnostics --neurons 20000 impulse
    python -m experiments.mioba.m1a.diagnostics --neurons 20000 persist
    python -m experiments.mioba.m1a.diagnostics genomelink
    python -m experiments.mioba.m1a.diagnostics --device cuda:0 prodscale
    python -m experiments.mioba.m1a.diagnostics --out <dir> plots
"""
from __future__ import annotations

import argparse
import json
import time
from pathlib import Path

import torch

from ..development.phenotype import develop
from ..fba.torch_backend import TorchBackend
from ..genome.schema import (ArtificialOrgan, Attachment, Genome,
                             ParameterMutation, fba0_genome)

DEFAULT_OUT = Path(__file__).resolve().parents[1] / ".runs" / "m1a"


# ---------------------------------------------------------------- helpers
def env_drive(n_base: int, frac: float = 0.01, hz: float = 50.0) -> dict:
    """The production input drive: ``frac`` of base neurons at ``hz``."""
    n_stim = max(1, int(round(n_base * frac)))
    return {"rates_hz": {f"slice:0-{n_stim}": hz}, "silence": []}


def make_backend(n: int, device: str = "cpu", seed: int = 0,
                 edges: int | None = None, connectivity: float = 0.01,
                 params: dict | None = None, batch_size: int = 1,
                 genome: Genome | None = None,
                 propagation_backend: str = "event_csc") -> TorchBackend:
    """Initialise a TorchBackend at scale ``n`` with optional param
    overrides (diagnostic only — not a genome mutation)."""
    b = TorchBackend(synthetic=True, synthetic_neurons=n,
                     connectivity=connectivity, synthetic_edges=edges,
                     topology_cache=False,
                     propagation_backend=propagation_backend)
    g = genome if genome is not None else fba0_genome(seed=seed)
    phen = develop(g, base_neurons=n)
    if params:
        phen = dict(phen)
        phen["params"] = {**phen.get("params", {}), **params}
    b.initialize(phen, batch_size=batch_size, seed=seed, device=device)
    return b


def ablate_recurrent(b: TorchBackend, factor: float) -> None:
    """Multiply every recurrent weight by ``factor`` without rebuilding:
    the base graph's values are raw and scaled by ``_base_scale`` at
    propagation time; the per-genome graph gets ``scaled``."""
    b._base_scale = float(b._base_scale) * factor
    if b.W_extra is not None:
        b.W_extra = b.W_extra.scaled(factor)


def wipe_neural_state(b: TorchBackend) -> None:
    """Reset v / g / delay line / refractory to their initial values while
    leaving spike bookkeeping and RNG alone (G4 control condition)."""
    b.v.fill_(b.params["v0"])
    b.g.zero_()
    b.delay_buf.zero_()
    b.refrac.fill_(b.params["tRefrac"])
    b.spikes.zero_()


def _gen_states(b: TorchBackend):
    return [g.get_state() for g in b._gens]


def _replay_input_events(b: TorchBackend, pre_states) -> tuple[int, int]:
    """Re-draw the uniform vector the just-finished step consumed and
    return ``(input_events, driven_neurons_hit)``. Restoring the pre-state
    then drawing leaves each generator exactly where the real step left
    it, so recording is side-effect free."""
    if b._drive_idx is None:
        return 0, 0
    events = 0
    hit = torch.zeros(b._drive_idx.numel(), dtype=torch.bool,
                      device=b.device)
    for g, st in zip(b._gens, pre_states):
        g.set_state(st)
        u = torch.rand((b._drive_idx.numel(),), device=b.device,
                       generator=g)
        h = u < b._drive_p
        events += int(h.sum().item())
        hit |= h
    return events, int(hit.sum().item())


def fresh_rec(raster: bool = False) -> dict:
    rec = {k: [] for k in (
        "delayed_norm", "delayed_mean", "delayed_max", "g_mean_pre",
        "v_mean_pre", "spikes", "spikes_driven", "spikes_nondriven",
        "input_events", "input_neurons_hit", "ext_current", "v_mean",
        "v_std", "g_mean", "g_max", "active_frac")}
    if raster:
        rec["raster"] = []          # per step: [neuron, ...] that spiked
    return rec


def step_record(b: TorchBackend, rec: dict, driven_hi: int = 0,
                raster: bool = False) -> None:
    """Run ONE real ``_one_step`` and append the forensic record."""
    p = b.params
    ptr = b._delay_ptr
    delayed = b.delay_buf[:, ptr, :]
    pre_states = _gen_states(b)
    rec["delayed_norm"].append(float(delayed.norm().item()))
    rec["delayed_mean"].append(float(delayed.mean().item()))
    rec["delayed_max"].append(float(delayed.max().item()))
    rec["g_mean_pre"].append(float(b.g.mean().item()))
    rec["v_mean_pre"].append(float(b.v.mean().item()))
    b.step(1)
    sp = b.spikes
    rec["spikes"].append(int(sp.sum().item()))
    if driven_hi or b._drive_idx is not None:
        m = getattr(b, "_m1a_dmask", None)
        if m is None:
            m = torch.zeros(b.n, dtype=torch.bool, device=b.device)
            m[b._drive_idx] = True
            b._m1a_dmask = m
        rec["spikes_driven"].append(
            int(sp.masked_select(m).sum().item()))
        rec["spikes_nondriven"].append(
            int(sp.masked_select(~m).sum().item()))
    if raster:
        rec["raster"].append(
            torch.nonzero(sp[0]).flatten().cpu().tolist())
    ev, hit = _replay_input_events(b, pre_states)
    rec["input_events"].append(ev)
    rec["input_neurons_hit"].append(hit)
    # magnitude the membrane actually saw per input event
    rec["ext_current"].append(ev * _stim_kick(b))
    rec["v_mean"].append(float(b.v.mean().item()))
    rec["v_std"].append(float(b.v.std().item()))
    rec["g_mean"].append(float(b.g.mean().item()))
    rec["g_max"].append(float(b.g.max().item()))
    rec["active_frac"].append(
        float((b.refrac >= p["tRefrac"]).float().mean().item()))


def _stim_kick(b: TorchBackend) -> float:
    """The voltage jump one external Poisson event applies to a driven
    neuron under this build's equations: ``scalePoisson`` alone for
    simulator semantics <= 2, ``wScale*scalePoisson`` from version 3."""
    p = b.params
    sem = b.semantics().get("simulator_semantics_version", 2)
    if sem >= 3:
        return float(p["wScale"]) * float(p["scalePoisson"])
    return float(p["scalePoisson"])


def run_traced(b: TorchBackend, n_steps: int,
               drive_on_until: int | None = None,
               wipe_each_step: bool = False,
               raster: bool = False,
               driven_hi: int = 0) -> dict:
    """Per-step forensic trace of a real run. ``drive_on_until`` turns
    the drive off at that step (impulse/pulse protocol)."""
    rec = fresh_rec(raster=raster)
    for t in range(int(n_steps)):
        if drive_on_until is not None and t == drive_on_until:
            b.set_inputs({"rates_hz": {}, "silence": []})
        step_record(b, rec, driven_hi=driven_hi, raster=raster)
        if wipe_each_step:
            wipe_neural_state(b)
    return rec


def divergence(a: dict, b: dict) -> dict:
    """Trajectory divergence between two traces (per-step spike counts)."""
    n = min(len(a["spikes"]), len(b["spikes"]))
    if not n:
        return {"l1": None, "diff_steps": None, "n_steps": 0}
    diff = [abs(a["spikes"][i] - b["spikes"][i]) for i in range(n)]
    return {"l1": int(sum(diff)),
            "diff_steps": int(sum(1 for d in diff if d > 0)),
            "n_steps": n,
            "total_a": int(sum(a["spikes"][:n])),
            "total_b": int(sum(b["spikes"][:n]))}


def first_spike(rec: dict, idx: int) -> int | None:
    for t, fired in enumerate(rec.get("raster") or []):
        if idx in fired:
            return t
    return None


def fixed_net(edges: list[tuple[int, int]], n: int, w: float,
              device: str = "cpu", params: dict | None = None,
              seed: int = 0) -> TorchBackend:
    """Backend over an explicit ``pre -> post`` edge list (deterministic,
    no sampled topology). The W override bypasses the base+extra split;
    the delay line and update equations are unchanged."""
    b = make_backend(n, device=device, seed=seed, connectivity=0.0,
                     params=params)
    if edges:
        post = torch.tensor([p for _, p in edges], dtype=torch.int64)
        pre = torch.tensor([q for q, _ in edges], dtype=torch.int64)
        W = torch.sparse_coo_tensor(torch.stack([post, pre]),
                                    torch.full((len(edges),), float(w)),
                                    (n, n))
        b.W = W.coalesce().to_sparse_csr()
    else:
        b.W = torch.sparse_coo_tensor((n, n)).to_sparse_csr()
    return b


def _save(out: Path, name: str, payload: dict) -> Path:
    out.mkdir(parents=True, exist_ok=True)
    p = out / name
    p.write_text(json.dumps(payload, indent=2, default=str))
    print(f"wrote {p}")
    return p


def _meta(b: TorchBackend) -> dict:
    return {"n": b.n, "n_base": b.n_base, "nnz": b.nnz,
            "steps_delay": b.steps_delay, "params": dict(b.params),
            "semantics": b.semantics(),
            "dataset": b.dataset_identity()}


# ---------------------------------------------------------------- §1 trace
def cmd_trace(args) -> dict:
    """Forensic per-step trace: shows what the simulator does with the
    Poisson input vs the recurrent current, and measures the echo."""
    out = {}
    for label, factor in (("baseline", 1.0), ("wzero", 0.0)):
        b = make_backend(args.neurons, device=args.device, seed=args.seed,
                         edges=args.edges, connectivity=args.connectivity,
                         params=_parse_params(args.param))
        if factor != 1.0:
            ablate_recurrent(b, factor)
        n_stim = max(1, int(round(b.n_base * args.stim_fraction)))
        b.set_inputs(env_drive(b.n_base, args.stim_fraction,
                               args.stim_hz))
        rec = run_traced(b, args.steps, driven_hi=n_stim)
        s = b.get_state_summary()
        out[label] = {
            "meta": _meta(b), "rec": rec, "steps": args.steps,
            "total_output_spikes": sum(rec["spikes"]),
            "total_input_events": sum(rec["input_events"]),
            "echo_ratio": (sum(rec["spikes"]) / max(1, sum(rec["input_events"]))),
            "mean_rate_hz": s["mean_rate_hz"],
            "driven_spikes": sum(rec["spikes_driven"]),
            "nondriven_spikes": sum(rec["spikes_nondriven"]),
            "n_driven": n_stim,
            "predicted_input_per_step":
                n_stim * args.stim_hz * b.params["dt"] / 1000.0,
        }
    out["divergence_wzero_vs_baseline"] = divergence(
        out["baseline"]["rec"], out["wzero"]["rec"])
    _save(Path(args.out), "trace.json", out)
    return out


# ---------------------------------------------------------------- §2 micro
def cmd_micro(args) -> dict:
    """Minimal deterministic networks, cases A–D."""
    out = {}
    device = args.device
    pulse_hz = 10000.0          # p=1.0 per step: deterministic drive

    # Case A — W = 0, pulse dies with the stimulus ----------------------
    b = fixed_net([], n=8, w=0.0, device=device)
    D = b.steps_delay
    b.set_inputs({"rates_hz": {"slice:0-2": pulse_hz}, "silence": []})
    rec = run_traced(b, 60, drive_on_until=10, raster=True)
    post = rec["spikes"][10 + D:]
    out["A_no_recurrence"] = {
        "desc": "W=0, drive 0-9 then off; activity must stop at once",
        "steps_delay": D, "spikes": rec["spikes"], "raster": rec["raster"],
        "pulse_spikes": sum(rec["spikes"][:10]),
        "post_pulse_spikes": sum(post),
        "pass": sum(post) == 0 and sum(rec["spikes"][:10]) > 0,
    }

    # Case B — feed-forward chain 0 -> 1 -> 2 -> 3 ----------------------
    b = fixed_net([(0, 1), (1, 2), (2, 3)], n=8, w=50.0, device=device)
    b.set_inputs({"rates_hz": {"slice:0-1": pulse_hz}, "silence": []})
    rec = run_traced(b, 400, drive_on_until=5, raster=True)
    onsets = {i: first_spike(rec, i) for i in range(8)}
    out["B_feedforward"] = {
        "desc": "chain 0->1->2->3, w=50, pulse on 0 for steps 0-4",
        "steps_delay": b.steps_delay, "spikes": rec["spikes"],
        "raster": rec["raster"],
        "first_spike_step": {str(k): v for k, v in onsets.items()},
        "pass": all(onsets[i] is not None for i in (1, 2, 3))
        and onsets[1] < onsets[2] < onsets[3],
    }

    # Case C — recurrent loop 0 -> 1 -> 2 -> 0 --------------------------
    b = fixed_net([(0, 1), (1, 2), (2, 0)], n=8, w=50.0, device=device)
    b.set_inputs({"rates_hz": {"slice:0-1": pulse_hz}, "silence": []})
    rec = run_traced(b, 400, drive_on_until=10, raster=True)
    post = rec["spikes"][10 + b.steps_delay:]
    out["C_recurrent_loop"] = {
        "desc": "loop 0->1->2->0, w=50, pulse 0-9; recurrent echo must "
                "return",
        "steps_delay": b.steps_delay, "spikes": rec["spikes"],
        "raster": rec["raster"],
        "post_pulse_spikes": sum(post),
        "post_pulse_active_steps": sum(1 for x in post if x > 0),
        "pass": sum(post) > 0,
    }

    # Case D — threshold manipulation -----------------------------------
    # w=20 -> ~5 mV integrated depolarisation per arrival: vThr=-49 lets
    # the chain reach neuron 3, vThr=-45 blocks it at neuron 1.
    res = {}
    for thr in (-49.0, -45.0):
        b = fixed_net([(0, 1), (1, 2), (2, 3)], n=8, w=20.0,
                      device=device, params={"vThr": thr})
        b.set_inputs({"rates_hz": {"slice:0-1": pulse_hz}, "silence": []})
        rec = run_traced(b, 400, drive_on_until=5, raster=True)
        res[str(thr)] = {"spikes": rec["spikes"],
                         "n1_first_spike": first_spike(rec, 1),
                         "n3_first_spike": first_spike(rec, 3)}
    out["D_threshold"] = {
        "desc": "chain w=20 pulse; vThr=-49 propagates past n1, -45 blocks",
        "runs": res,
        "pass": res["-49.0"]["n1_first_spike"] is not None
        and res["-45.0"]["n1_first_spike"] is None,
    }
    _save(Path(args.out), "micro.json", out)
    return out


# ------------------------------------------------------------- §3 ablation
ABLATION_CONDITIONS = (
    # (label, recurrent_factor, drive_steps)  drive_steps=None: whole run
    ("baseline",            1.0, None),
    ("no_recurrence",       0.0, None),
    ("strong_recurrence",  None, None),     # factor from --strong-factor
    ("no_input",            1.0, 0),
    ("pulse",               1.0, None),     # pulse via --pulse-steps
    ("pulse_no_recurrence", 0.0, None),
)


def cmd_ablation(args) -> dict:
    """Same topology/input/seed under six recurrent/drive conditions."""
    out = {"meta": None, "conditions": {}}
    for label, factor, drive_steps in ABLATION_CONDITIONS:
        f = args.strong_factor if label == "strong_recurrence" else factor
        pulse_until = (args.pulse_steps if label.startswith("pulse")
                       else drive_steps)
        b = make_backend(args.neurons, device=args.device, seed=args.seed,
                         edges=args.edges, connectivity=args.connectivity)
        if f != 1.0:
            ablate_recurrent(b, f)
        n_stim = max(1, int(round(b.n_base * args.stim_fraction)))
        b.set_inputs(env_drive(b.n_base, args.stim_fraction,
                               args.stim_hz))
        t0 = time.perf_counter()
        rec = run_traced(b, args.steps, drive_on_until=pulse_until,
                         driven_hi=n_stim)
        s = b.get_state_summary()
        out["conditions"][label] = {
            "factor": f, "pulse_until": pulse_until,
            "rec": rec, "wall_s": round(time.perf_counter() - t0, 3),
            "mean_rate_hz": s["mean_rate_hz"],
            "total_spikes": sum(rec["spikes"]),
            "nondriven_spikes": sum(rec["spikes_nondriven"]),
            "input_events": sum(rec["input_events"]),
            "mean_delayed_norm": (sum(rec["delayed_norm"])
                                  / max(1, len(rec["delayed_norm"]))),
        }
        out["meta"] = out["meta"] or _meta(b)
    base = out["conditions"]["baseline"]["rec"]
    for label, c in out["conditions"].items():
        c["divergence_vs_baseline"] = divergence(base, c["rec"])
    _save(Path(args.out), "ablation.json", out)
    return out


# ---------------------------------------------------------- G2 parameter
def cmd_sweep(args) -> dict:
    """Extreme-value parameter sweep: does each genome-mutable parameter
    move activity at all? Reports the trajectory, not just the rate."""
    sweeps = {
        "vThr": [-200.0, -52.0, -46.0, -45.0, -40.0, 0.0, 20.0, 80.0, 200.0, 400.0],
        "wScale": [0.0, 0.05, 0.1, 0.275, 5.0, 20.0, 60.0, 200.0],
        "tauMem": [2.0, 5.0, 20.0, 60.0, 200.0],
        "tauSyn": [1.0, 5.0, 20.0, 50.0],
        "tRefrac": [0.1, 1.0, 2.2, 10.0, 50.0],
        "scalePoisson": [5.0, 25.0, 68.75, 250.0, 2500.0],
    }
    if args.only:
        sweeps = {k: v for k, v in sweeps.items() if k in args.only.split(",")}
    baseline_rec = None
    out = {"meta": None, "sweeps": {}}
    for param, values in sweeps.items():
        rows = []
        for val in values:
            b = make_backend(args.neurons, device=args.device,
                             seed=args.seed, edges=args.edges,
                             connectivity=args.connectivity,
                             params={param: val})
            b.set_inputs(env_drive(b.n_base, args.stim_fraction,
                                   args.stim_hz))
            rec = run_traced(b, args.steps)
            s = b.get_state_summary()
            row = {"value": val,
                   "mean_rate_hz": s["mean_rate_hz"],
                   "total_spikes": sum(rec["spikes"]),
                   "nondriven_spikes": sum(rec["spikes_nondriven"]),
                   "input_events": sum(rec["input_events"]),
                   "mean_delayed_norm": (sum(rec["delayed_norm"])
                                         / max(1, len(rec["delayed_norm"]))),
                   "spikes_per_step": rec["spikes"]}
            rows.append(row)
            if param == "wScale" and val == 0.275:
                baseline_rec = rec
            if param == "vThr" and val == -45.0:
                baseline_rec = rec
            if param == "tauMem" and val == 20.0:
                baseline_rec = rec
        for row in rows:
            row["divergence_vs_default"] = (
                divergence(baseline_rec, {"spikes": row["spikes_per_step"]})
                if baseline_rec else None)
        out["sweeps"][param] = rows
        out["meta"] = out["meta"] or _meta(b)
    _save(Path(args.out), "sweep.json", out)
    return out


# ------------------------------------------------------------ G3 impulse
def cmd_impulse(args) -> dict:
    """Pulse the drive for ``pulse_steps`` then zero input; record what
    the network does on its own afterwards, at several wScale values."""
    out = {"meta": None, "runs": {}, "pulse_steps": args.pulse_steps}
    for wscale in args.wscales:
        b = make_backend(args.neurons, device=args.device, seed=args.seed,
                         edges=args.edges, connectivity=args.connectivity,
                         params={"wScale": wscale})
        n_stim = max(1, int(round(b.n_base * args.stim_fraction)))
        b.set_inputs(env_drive(b.n_base, args.stim_fraction,
                               args.stim_hz))
        rec = run_traced(b, args.steps,
                         drive_on_until=args.pulse_steps,
                         driven_hi=n_stim)
        post = rec["spikes"][args.pulse_steps:]
        out["runs"][str(wscale)] = {
            "rec": rec,
            "post_pulse_spikes": sum(post),
            "post_pulse_active_steps": sum(1 for x in post if x > 0),
            "post_pulse_nondriven": sum(rec["spikes_nondriven"][args.pulse_steps:]),
        }
        out["meta"] = out["meta"] or _meta(b)
    _save(Path(args.out), "impulse.json", out)
    return out


# ------------------------------------------------- G4 state persistence
def cmd_persist(args) -> dict:
    """Normal run vs neural state wiped every step — same input/seed."""
    out = {}
    for label, wipe in (("normal", False), ("state_reset", True)):
        b = make_backend(args.neurons, device=args.device, seed=args.seed,
                         edges=args.edges, connectivity=args.connectivity)
        n_stim = max(1, int(round(b.n_base * args.stim_fraction)))
        b.set_inputs(env_drive(b.n_base, args.stim_fraction,
                               args.stim_hz))
        rec = run_traced(b, args.steps, wipe_each_step=wipe,
                         driven_hi=n_stim)
        out[label] = {"rec": rec, "total_spikes": sum(rec["spikes"]),
                      "mean_rate_hz": b.get_state_summary()["mean_rate_hz"]}
    out["divergence"] = divergence(out["normal"]["rec"],
                                   out["state_reset"]["rec"])
    out["meta"] = _meta(b)
    _save(Path(args.out), "persist.json", out)
    return out


# ------------------------------------------- §4 genome->activity linkage
def cmd_genomelink(args) -> dict:
    """Trace genome -> decoded phenotype -> simulator tensor -> activity
    for representative parameter mutations."""
    mutations = [
        ("wScale", "scale", 60.0),
        ("vThr", "add", 20.0),          # -45 -> -25 mV
        ("vThr", "scale", 0.5),         # -45 -> -22.5 mV
        ("tauMem", "scale", 0.25),
        ("tRefrac", "scale", 10.0),
    ]
    rows = []
    base_g = fba0_genome(seed=args.seed)
    base_phen = develop(base_g, base_neurons=args.neurons)
    for i, (path, op, value) in enumerate(mutations):
        g = fba0_genome(seed=args.seed)
        g.parameter_mutations.append(ParameterMutation(
            mutation_id=f"m1a_{i}", path=path, op=op, value=value,
            scope="global"))
        g = g.finalize()
        phen = develop(g, base_neurons=args.neurons)
        b = make_backend(args.neurons, device=args.device, seed=args.seed,
                         genome=g)
        n_stim = max(1, int(round(b.n_base * args.stim_fraction)))
        b.set_inputs(env_drive(b.n_base, args.stim_fraction,
                               args.stim_hz))
        rec = run_traced(b, args.steps)
        s = b.get_state_summary()
        rows.append({
            "genome_id": g.genome_id,
            "mutation": {"path": path, "op": op, "value": value},
            "decoded_phenotype_param": phen["params"].get(path),
            "baseline_param": base_phen["params"].get(path),
            "simulator_tensor_value": b.params.get(path),
            "mean_rate_hz": s["mean_rate_hz"],
            "total_spikes": sum(rec["spikes"]),
            "nondriven_spikes": sum(rec["spikes_nondriven"]),
        })
    # baseline run for comparison
    b = make_backend(args.neurons, device=args.device, seed=args.seed,
                     genome=base_g)
    b.set_inputs(env_drive(b.n_base, args.stim_fraction, args.stim_hz))
    rec0 = run_traced(b, args.steps)
    base_total = sum(rec0["spikes"])
    for row in rows:
        row["baseline_total_spikes"] = base_total
        row["changed_activity"] = row["total_spikes"] != base_total
    out = {"baseline": {"mean_rate_hz":
                        b.get_state_summary()["mean_rate_hz"],
                        "total_spikes": base_total,
                        "input_events": sum(rec0["input_events"]),
                        "params": dict(b.params)},
           "mutations": rows, "meta": _meta(b)}
    _save(Path(args.out), "genomelink.json", out)
    return out


# ------------------------------------------------ G5 production scale
def cmd_prodscale(args) -> dict:
    """Production-scale reproduction: 139k neurons / 14M edges, on the
    requested device. Baseline vs W=0 vs strong-W vs extreme vThr vs
    impulse, per-step recorded."""
    out = {"meta": None, "runs": {}}
    cases = [
        ("baseline",        {},                        None),
        ("no_recurrence",   {"_ablate": 0.0},          None),
        ("strong_wscale",   {"wScale": args.strong_wscale}, None),
        ("vThr_high",       {"vThr": args.high_vthr},  None),
        ("impulse_default", {},                        args.pulse_steps),
        ("impulse_strong",  {"wScale": args.strong_wscale}, args.pulse_steps),
    ]
    for label, params, pulse_until in cases:
        ablate = params.pop("_ablate", None)
        b = make_backend(args.neurons, device=args.device, seed=args.seed,
                         edges=args.edges, connectivity=args.connectivity,
                         params=params or None)
        if ablate is not None:
            ablate_recurrent(b, ablate)
        n_stim = max(1, int(round(b.n_base * args.stim_fraction)))
        b.set_inputs(env_drive(b.n_base, args.stim_fraction,
                               args.stim_hz))
        t0 = time.perf_counter()
        rec = run_traced(b, args.steps, drive_on_until=pulse_until,
                         driven_hi=n_stim)
        wall = time.perf_counter() - t0
        s = b.get_state_summary()
        out["runs"][label] = {
            "rec": rec, "wall_s": round(wall, 3),
            "mean_rate_hz": s["mean_rate_hz"],
            "total_spikes": sum(rec["spikes"]),
            "nondriven_spikes": sum(rec["spikes_nondriven"]),
            "input_events": sum(rec["input_events"]),
            "post_pulse_spikes": (sum(rec["spikes"][pulse_until:])
                                  if pulse_until is not None else None),
        }
        out["meta"] = _meta(b)
    base = out["runs"]["baseline"]["rec"]
    for label, r in out["runs"].items():
        r["divergence_vs_baseline"] = divergence(base, r["rec"])
    _save(Path(args.out), "prodscale.json", out)
    return out


# ------------------------------------------------------------------ plots
def cmd_plots(args) -> None:
    """Render the required M1A figures from the JSON artifacts in --out."""
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    out = Path(args.out)
    figs = out / "plots"
    figs.mkdir(parents=True, exist_ok=True)

    def savefig(name):
        plt.tight_layout()
        plt.savefig(figs / name, dpi=140)
        plt.close()
        print(f"wrote {figs / name}")

    trace = _load(out / "trace.json")
    abl = _load(out / "ablation.json")
    sweep = _load(out / "sweep.json")
    imp = _load(out / "impulse.json")
    per = _load(out / "persist.json")
    gl = _load(out / "genomelink.json")
    prod = _load(out / "prodscale.json")
    micro = _load(out / "micro.json")

    # 1. external input vs output activity
    if trace:
        rec = trace["baseline"]["rec"]
        plt.figure(figsize=(8, 4))
        plt.plot(rec["input_events"], label="Poisson input events/step",
                 lw=1)
        plt.plot(rec["spikes"], label="network output spikes/step", lw=1)
        if rec.get("spikes_nondriven"):
            plt.plot(rec["spikes_nondriven"],
                     label="non-driven output spikes/step", lw=1)
        plt.xlabel("step (0.1 ms)"); plt.ylabel("events / step")
        plt.legend(); plt.title("M1A-1 external input vs output activity")
        savefig("01_input_vs_output.png")

    # 2. recurrent current vs external current
    if trace:
        rec = trace["baseline"]["rec"]
        plt.figure(figsize=(8, 4))
        plt.plot(rec["delayed_norm"], label="recurrent current norm (arriving)", lw=1)
        plt.plot(rec["ext_current"], label="external drive magnitude/step (mV)", lw=1)
        plt.xlabel("step"); plt.ylabel("mV")
        plt.legend(); plt.title("M1A-2 recurrent vs external current")
        savefig("02_recurrent_vs_external.png")

    # 3. pulse-response raster (micro loop)
    if micro and "C_recurrent_loop" in micro:
        raster = micro["C_recurrent_loop"]["raster"]
        plt.figure(figsize=(8, 4))
        for t, fired in enumerate(raster):
            for nrn in fired:
                plt.plot(t, nrn, "k.", ms=2)
        plt.axvline(10, color="r", ls="--", lw=0.8, label="drive off")
        plt.xlabel("step"); plt.ylabel("neuron")
        plt.legend(); plt.title("M1A-3 pulse-response raster (loop 0-1-2-0)")
        savefig("03_pulse_raster.png")

    # 4. population activity after pulse
    if imp:
        plt.figure(figsize=(8, 4))
        for name, r in imp["runs"].items():
            plt.plot(r["rec"]["spikes"], lw=1, label=f"wScale={name}")
        plt.axvline(int(imp.get("pulse_steps", 10)), color="r", ls="--",
                    lw=0.8)
        plt.xlabel("step"); plt.ylabel("spikes / step")
        plt.legend(); plt.title("M1A-4 population activity after pulse")
        savefig("04_post_pulse_activity.png")

    # 5. W=0 / normal / strong-W comparison
    if abl:
        plt.figure(figsize=(8, 4))
        for label in ("baseline", "no_recurrence", "strong_recurrence"):
            c = abl["conditions"].get(label)
            if c:
                plt.plot(c["rec"]["spikes"], lw=1, label=label)
        plt.xlabel("step"); plt.ylabel("spikes / step")
        plt.legend(); plt.title("M1A-5 recurrent ablation comparison")
        savefig("05_ablation.png")

    # 6. threshold sweep
    if sweep and "vThr" in sweep["sweeps"]:
        rows = sweep["sweeps"]["vThr"]
        plt.figure(figsize=(6, 4))
        plt.plot([r["value"] for r in rows],
                 [r["mean_rate_hz"] for r in rows], "o-")
        plt.xlabel("vThr (mV)"); plt.ylabel("mean rate (Hz)")
        plt.title("M1A-6 threshold sweep"); savefig("06_vthr_sweep.png")

    # 7. tau sweep
    if sweep and "tauMem" in sweep["sweeps"]:
        rows = sweep["sweeps"]["tauMem"]
        plt.figure(figsize=(6, 4))
        plt.plot([r["value"] for r in rows],
                 [r["mean_rate_hz"] for r in rows], "o-")
        plt.xlabel("tauMem (ms)"); plt.ylabel("mean rate (Hz)")
        plt.title("M1A-7 tauMem sweep"); savefig("07_tau_sweep.png")

    # 8. genome parameter -> activity sensitivity
    if gl:
        rows = gl["mutations"]
        plt.figure(figsize=(7, 4))
        labels = [f"{r['mutation']['path']} {r['mutation']['op']} "
                  f"{r['mutation']['value']}" for r in rows]
        vals = [r["mean_rate_hz"] for r in rows]
        plt.bar(range(len(rows)), vals)
        plt.axhline(gl["baseline"]["mean_rate_hz"], color="r", ls="--",
                    lw=0.8, label="baseline")
        plt.xticks(range(len(rows)), labels, rotation=30, ha="right")
        plt.ylabel("mean rate (Hz)"); plt.legend()
        plt.title("M1A-8 genome parameter -> activity")
        savefig("08_genome_sensitivity.png")

    # 9. normal-state vs state-reset
    if per:
        plt.figure(figsize=(8, 4))
        plt.plot(per["normal"]["rec"]["spikes"], lw=1, label="normal")
        plt.plot(per["state_reset"]["rec"]["spikes"], lw=1,
                 label="state wiped each step")
        plt.xlabel("step"); plt.ylabel("spikes / step"); plt.legend()
        plt.title("M1A-9 state persistence"); savefig("09_state_reset.png")

    # 10. device throughput
    dev = _load(out / "device_bench.json")
    if dev:
        rows = [r for r in dev["rows"] if r.get("ok")]
        plt.figure(figsize=(6, 4))
        plt.bar([r["device"] for r in rows],
                [r["evaluations_per_minute"] for r in rows])
        plt.ylabel("successful evaluations / min")
        plt.title("M1A-10 device throughput"); savefig("10_devices.png")

    # 11. production-scale causality
    if prod:
        plt.figure(figsize=(8, 4))
        for label in ("baseline", "no_recurrence", "strong_wscale",
                      "vThr_high", "impulse_default", "impulse_strong"):
            r = prod["runs"].get(label)
            if r:
                plt.plot(r["rec"]["spikes"], lw=1, label=label)
        plt.xlabel("step"); plt.ylabel("spikes / step"); plt.legend()
        plt.title("M1A-11 production-scale causality (139k/14M)")
        savefig("11_prodscale.png")


def _load(p: Path):
    if p.is_file():
        return json.loads(p.read_text())
    return None


def _parse_params(items) -> dict:
    out = {}
    for it in items or []:
        k, v = it.split("=", 1)
        out[k] = float(v)
    return out


# -------------------------------------------------------------------- cli
def build_parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=
                                 argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    ap.add_argument("--device", default="cpu")
    ap.add_argument("--neurons", type=int, default=20000)
    ap.add_argument("--edges", type=int, default=None)
    ap.add_argument("--connectivity", type=float, default=0.01)
    ap.add_argument("--steps", type=int, default=500)
    ap.add_argument("--seed", type=int, default=0)
    ap.add_argument("--stim-fraction", type=float, default=0.01)
    ap.add_argument("--stim-hz", type=float, default=50.0)
    ap.add_argument("--param", action="append",
                    help="param=value override, repeatable")
    sub = ap.add_subparsers(dest="cmd", required=True)

    sub.add_parser("trace")

    sub.add_parser("micro")

    p = sub.add_parser("ablation")
    p.add_argument("--strong-factor", type=float, default=60.0)
    p.add_argument("--pulse-steps", type=int, default=50)

    p = sub.add_parser("sweep")
    p.add_argument("--only", default=None)

    p = sub.add_parser("impulse")
    p.add_argument("--pulse-steps", type=int, default=10)
    p.add_argument("--wscales", type=float, nargs="+",
                   default=[0.275, 20.0, 60.0])

    sub.add_parser("persist")

    sub.add_parser("genomelink")

    p = sub.add_parser("prodscale")
    p.add_argument("--strong-wscale", type=float, default=60.0)
    p.add_argument("--high-vthr", type=float, default=20.0)
    p.add_argument("--pulse-steps", type=int, default=50)

    sub.add_parser("plots")
    return ap


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    fn = {"trace": cmd_trace, "micro": cmd_micro, "ablation": cmd_ablation,
          "sweep": cmd_sweep, "impulse": cmd_impulse,
          "persist": cmd_persist, "genomelink": cmd_genomelink,
          "prodscale": cmd_prodscale, "plots": cmd_plots}[args.cmd]
    fn(args)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
