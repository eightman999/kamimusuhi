"""In-process interruption engine for C0.

run_leg()      : step a batch of envs + runner from current t to t_end,
                 with forced ANSWER at query deadlines.
capture_payloads / payloads_to_states : artifact semantics without the disk
                 (same codec path the subprocess phases use).
run_interrupted: full episode(s) through all configured interruptions --
                 the fast equivalent of the phase_a -> phase_b process chain.
"""
from __future__ import annotations

import numpy as np

from ..env.c0_env import ANSWER
from .codec import (apply_condition, decode_payload, mask_state,
                    shuffle_state)


def run_leg(envs, runner, t_end: int, greedy: bool = True,
            record_latents: bool = False, latents: list | None = None):
    """Run all envs (lockstep schedule) until env.t == t_end."""
    while envs[0].t < t_end:
        obs = np.stack([e._obs() for e in envs])
        acts, ans = runner.act(obs, greedy=greedy, envs=envs)
        for i, e in enumerate(envs):
            if e.pending is not None and e.t == e.pending["deadline"]:
                acts[i] = ANSWER
        for i, e in enumerate(envs):
            e.step(int(acts[i]), int(ans[i]))
        if record_latents and latents is not None:
            h = runner.hidden()
            latents.append(None if h is None else h.copy())


def capture_payloads(runner, condition: str, budget: int | None,
                     spec: list[tuple]) -> list[bytes]:
    return [apply_condition(st, condition, spec, budget)
            for st in runner.get_states()]


def payloads_to_states(payloads: list[bytes], condition: str,
                       spec: list[tuple]) -> list[dict]:
    return [decode_payload(p, condition, spec) for p in payloads]


def run_interrupted(runner, envs, spec: list[tuple], condition: str = "full",
                    budget: int | None = None, *,
                    donor_payloads: list[bytes] | None = None,
                    mask_frac: float | None = None,
                    shuffle: bool = False,
                    rng: np.random.Generator | None = None,
                    greedy: bool = True,
                    record_latents: bool = False,
                    uninterrupted: bool = False) -> dict:
    """Run the episode through every configured interruption.

    At each stop the runner state is captured, serialized through the
    condition codec, optionally perturbed (donor swap / mask / shuffle),
    restored, and the episode resumes at resume = stop + gap.

    uninterrupted=True skips capture/restore entirely: the reference run.
    """
    rng = rng if rng is not None else np.random.default_rng(0)
    runner.reset(len(envs))
    ivals = envs[0].cfg.intervals()
    latents: list = []
    resume_h_pre = None          # hidden right after restore, at resume step
    for li, (lo, hi) in enumerate(ivals):
        if li > 0:
            for e in envs:
                e.advance_to(lo)
            if not uninterrupted:
                payloads = capture_payloads(runner, condition, budget, spec)
                if donor_payloads is not None:
                    payloads = donor_payloads
                states = payloads_to_states(payloads, condition, spec)
                if mask_frac:
                    states = [mask_state(s, mask_frac, rng, spec)
                              for s in states]
                if shuffle:
                    states = [shuffle_state(s, rng, spec) for s in states]
                runner.set_states(states)
                h = runner.hidden()
                resume_h_pre = None if h is None else h.copy()
        run_leg(envs, runner, hi, greedy,
                record_latents=record_latents, latents=latents)
    records = [list(e.query_records) for e in envs]
    stats = [e.episode_stats() for e in envs]
    out = {"records": records, "stats": stats}
    if record_latents:
        out["latents"] = latents
        out["resume_hidden"] = resume_h_pre
    return out
