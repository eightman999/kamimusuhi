"""Running one episode under a virtual environment (M1 §5-§7, §9.1).

M0's evaluation was a single uninterrupted run whose only output was a
mean firing rate. That cannot answer the question M1 asks — *does this
individual notice a crisis, and can it come back from one?* — because it
has no time axis and no crisis.

An episode here is a sequence of slices. Between slices the environment
advances: channels move, debt accumulates, consequences bite (noise,
dropout, stale perception, a shrinking compute budget). Each slice is
scored on its own, so the run produces a performance *trace* rather than
a single number, and the trace is what makes

    performance_before / during / after,  recovery_time

measurable at all. Recovery is deliberately separated from resistance: an
individual that never wobbles and one that wobbles and recovers are
different organisms, and M1 wants to be able to tell them apart.

The task is the one M0 already implied — hold the population near
``target_rate_hz`` — but now it must be held *through* something. Slices
lost to an early termination are scored as failures, so letting debt run
away can never be a cheap way to end an episode early.
"""
from __future__ import annotations


def _rate(delta_spikes: float, n_neurons: int, seconds: float) -> float:
    if n_neurons <= 0 or seconds <= 0:
        return 0.0
    return delta_spikes / (n_neurons * seconds)


def _lane_totals(backend) -> list[int]:
    s = backend.get_state_summary()
    return [int(x) for x in s["per_batch_spike_counts"]]


def run_episode(backend, envs, duration_ms: float, target_rate_hz: float,
                task_rate_hz: float, timer=None) -> dict:
    """Run one chunk of lanes through their environments.

    ``envs`` is one :class:`VirtualEnvironment` per lane, seeded from that
    replicate's disturbance seed. The lanes share a backend (they are the
    batch dimension), so the *simulated time* of a slice is shared; a lane
    whose compute budget has collapsed is charged by scoring its slice on
    the shortened activity it managed, not by desynchronising the batch.

    Returns per-lane slice traces plus the disturbance-response summary.
    """
    n_lanes = len(envs)
    slices = max(1, min(e.n_slices for e in envs))
    slice_ms = float(duration_ms) / slices
    n_neurons = getattr(backend, "n", None) or 1

    prev = _lane_totals(backend)
    traces: list[list[dict]] = [[] for _ in range(n_lanes)]
    terminated = [False] * n_lanes
    sim_ms = 0.0

    for i in range(slices):
        t_frac = i / slices
        states = [env.advance(t_frac) for env in envs]
        cons = [env.consequences() for env in envs]

        for lane, c in enumerate(cons):
            if c["terminate"]:
                terminated[lane] = True

        if all(terminated):
            break

        # the batch shares simulated time; the lane with the tightest
        # compute budget sets how much of this slice is actually run, and
        # the others are scored on the same shortened window
        compute = min(c["compute_fraction"] for c in cons)
        run_ms = max(slice_ms * compute, slice_ms * 0.05)

        # lane 0's drive is applied to the batch: the backend takes one
        # drive for all lanes, so per-lane environments must agree on the
        # stimulus layout (they do: same config, same neuron ranges) and
        # differ in their sampled crises, which show through the lane's
        # own noise/dropout draw
        drive = envs[0].drive(task_rate_hz)
        for env in envs[1:]:
            env.drive(task_rate_hz)      # keep each lane's RNG in step
        backend.set_inputs(drive)
        if timer is not None:
            with timer.phase("simulation_loop"):
                backend.run(run_ms)
        else:
            backend.run(run_ms)
        sim_ms += run_ms

        now = _lane_totals(backend)
        for lane in range(n_lanes):
            delta = now[lane] - prev[lane]
            rate = _rate(delta, n_neurons, run_ms / 1000.0)
            st = states[lane]
            traces[lane].append({
                "slice": i,
                "t_frac": round(t_frac, 4),
                "rate_hz": round(rate, 6),
                "score": round(-abs(rate - target_rate_hz), 6),
                "disturbed": bool(st.active),
                "kinds": list(st.active),
                "debt": round(st.debt, 4),
                "terminated": terminated[lane],
                "compute_fraction": cons[lane]["compute_fraction"],
                "sensor_noise": cons[lane]["sensor_noise"],
            })
        prev = now

    # slices never run (early termination) are failures, not absences
    worst = -float(target_rate_hz)
    for lane in range(n_lanes):
        for i in range(len(traces[lane]), slices):
            traces[lane].append({
                "slice": i, "t_frac": round(i / slices, 4),
                "rate_hz": None, "score": worst, "disturbed": True,
                "kinds": ["terminated"], "debt": envs[lane].state.debt,
                "terminated": True, "compute_fraction": 0.0,
                "sensor_noise": None, "missing": True,
            })

    return {
        "slices": slices,
        "slice_ms": round(slice_ms, 4),
        "simulated_ms": round(sim_ms, 4),
        "per_lane": [analyse_trace(traces[i], envs[i], target_rate_hz)
                     for i in range(n_lanes)],
        "environments": [env.summary() for env in envs],
    }


def analyse_trace(trace: list[dict], env, target_rate_hz: float) -> dict:
    """Turn one lane's slice trace into the §9.1 response measures."""
    clean = [s for s in trace if not s["disturbed"]]
    disturbed = [s for s in trace if s["disturbed"]]

    def mean(rows, key="score"):
        vals = [r[key] for r in rows if r.get(key) is not None]
        return (sum(vals) / len(vals)) if vals else None

    windows = []
    for ev in env.schedule.events:
        before = [s for s in trace if s["t_frac"] < ev.start_frac
                  and not s["disturbed"]]
        during = [s for s in trace
                  if ev.start_frac <= s["t_frac"] < ev.end_frac]
        after = [s for s in trace if s["t_frac"] >= ev.end_frac]
        b, d = mean(before), mean(during)
        # recovery: slices after the crisis ends until the score is back
        # within 10% of the pre-crisis level. None = never recovered
        recovery = None
        if b is not None:
            tolerance = abs(b) * 0.1 + 1e-6
            for k, s in enumerate(after):
                if s["score"] >= b - tolerance:
                    recovery = k
                    break
        windows.append({
            "kind": ev.kind, "severity": ev.severity,
            "compound_with": ev.compound_with,
            "start_frac": ev.start_frac, "end_frac": ev.end_frac,
            "performance_before": b,
            "performance_during": d,
            "performance_after": mean(after),
            "impact": (None if (b is None or d is None) else round(b - d, 6)),
            "recovery_slices": recovery,
            "recovered": recovery is not None,
        })

    return {
        "n_slices": len(trace),
        "task_score_clean": mean(clean),
        "task_score_all": mean(trace),
        "mean_rate_hz": mean(trace, "rate_hz"),
        "disturbed_slices": len(disturbed),
        "missing_slices": sum(1 for s in trace if s.get("missing")),
        "terminated_early": bool(env.state.terminated),
        "final_debt": round(env.state.debt, 4),
        "peak_debt": round(max((s["debt"] for s in trace), default=0.0), 4),
        "disturbances": windows,
        "trace": trace,
    }
