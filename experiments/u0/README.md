# Kamimusuhi U0 — Need-Guided Memory

Can an agent with finite memory learn that the *same event* is worth
remembering or not depending on its **own internal need state** — because
stored events have later homeostatic consequences?

This experiment connects H0 (artificial homeostasis) and R0 (learned
memory gate): the agent must keep internal variables in their preferred
ranges *and* decide what to keep in a 4-slot memory, but unlike R0 the
value of an event is not fixed by its type — it is conditioned on which
variable is likely to crisis later.

Isolated from M/H/S/T/J systems. Standalone under `experiments/u0/`.

## Scientific question

```text
same event, same world, same history
        different internal need state
                    ↓
        different STORE probability
                    ↓
        different later homeostatic outcome
```

The target phenomenon is **need-conditioned memory gating**, not accuracy,
not recall fidelity, and not "autobiographical memory". A positive result
means a learned gating mechanism that correlates storage decisions with
internal state in a way that causally improves later regulation.

## Why a classifier cannot solve it

Event types alone do not determine value:

- `RESOURCE@C` is valuable when energy is (or drifts) low, irrelevant when
  energy is comfortable;
- `SHELTER@A` matters under cold drift, not under stable temperature;
- per-episode location↔function assignment changes, so fixed
  location/type memorization is penalized (and explicitly tested by
  `event_perm` / `need_mapping_shift`).

Eight functional event reports exceed the 4 memory slots, so "store
everything useful" is impossible; the correct subset depends on which
variables are vulnerable. Need identities are **never labeled**: needs are
sampled per episode weighted by the reset-time deviation profile
(`need_beta` softmax + `need_floor`), so the only available signal is the
correlation between the agent's observed internal state and the identity
of the coming crisis.

## Environment

- 6-location ring (A–F). `MOVE` advances one step; positions are
  observed as sin/cos so the encoding scales with the ring size.
- Internal state: `energy`, `temperature`, `risk`, `certainty` with
  preferred ranges energy [0.40, 0.80], temperature [0.35, 0.65],
  risk [0.00, 0.30], certainty [0.40, 1.00]. Death: energy < 0.05,
  temperature outside [0.05, 0.95], risk > 0.98.
- Sites: each of the 4 functions (resource→energy, shelter→temperature,
  safe_zone→risk, obs_point→certainty) exists at 2 locations — one
  `potent` (1.0) and one `meager` (0.45) site. `ACT` at a matching site
  during a crisis stops the drain and recovers by `potency × gap`;
  `ACT` elsewhere costs energy and raises risk (trial-and-error
  fallback exists: memory is the fast path, not the only path — but
  during a fast crisis, wandering the ring is often lethal).
- Events: 8 functional reports (truthful type+location+potency) early in
  the episode, plus DISTRACTOR (typed junk) and NOISE items.
- Needs: 2 per episode; onset = last functional event + U[24,80], second
  need +U[8,24] later. While active, the variable drifts toward its
  lethal bound (drain 0.03/step for energy/certainty, 0.02 for
  temperature/risk).
- Observation: 24 dims — internal(4) + position sin/cos(2) + event
  type one-hot(7) + event location sin/cos(2) + event potency(1) +
  memory occupancy(1) + crisis flag(1) + recall readout valid(1) +
  recall target sin/cos(2) + active-need count(1) + crisis age(1) +
  time(1).
- RECALL readout answers "where is the best stored site for the
  currently needed function" — a derived feature, because the experiment
  is about *gating*, not readout mechanics.

Actions (6): `IGNORE, STORE, RECALL, WAIT, MOVE, ACT`.
Costs: STORE 0.005, RECALL 0.005 (reward); MOVE/ACT cost energy;
WAIT rests (slow partial recovery).

```text
reward = -homeostatic_error - memory_operation_cost - survival_violation
```

No signal marks an event as relevant; `episode_stats` records ground
truth for analysis only.

## Models / baselines

- Learned: `mlp` (256 hidden), `gru64`, `gru128` — single 6-way action
  head + value head; GRU state resets each episode.
- Baselines share one crisis-recovery routine and differ only in the
  storage rule: `random` (p=0.2), `fifo`, `lru`, `store_all`,
  `heuristic_current_need` (stores events for *currently* deviated
  variables — myopic), `oracle` (stores exactly the potent sites of the
  to-be-needed functions — upper bound).

## Metrics

`error_full` (episode-integrated homeostatic error; post-death steps
count at the death error), `survival_fraction`, `stable_fraction`,
`need_resolution`, `important_retention` (needed-function site present at
need onset), `store_precision` (stores matching fired needs / all
stores), action counts, `oracle_gap`.

## Causal tests

| id | manipulation | expectation if memory is causal |
|----|--------------|--------------------------------|
| U-C1 | `erase` memory at need onset | error ↑ ≥30% |
| U-C2 | `shuffle`: swap with a donor episode's memory | error ↑ |
| U-C3 | `need_intervention`: pin internal state during the event window, identical event streams | P(STORE\|func) shifts ≥0.20 between adverse and safe states |
| U-C4 | `event_perm`: permute observed type/location channels | breaks channel-position memorization |

## OOD conditions

`delay96`, `delay128`, `delay160` (episode length extended),
`distractor2x`, `distractor4x`, `need_mapping_shift` (function→variable
rotation: sites still heal, the association moves), `event_perm`.

## Pre-registered gates (recorded before seeing results)

| gate | criterion | threshold |
|------|-----------|-----------|
| U-H1 | learned `error_full` below min(random, fifo) | ≥ 20% improvement |
| U-H2 | `store_precision` AND `important_retention` | both ≥ 0.75 |
| U-H3 | erase OR shuffle degradation | ≥ 30% `error_full` increase |
| U-H4 | need-intervention mean \|ΔP(STORE)\| | ≥ 0.20 |
| U-H5 | main conclusions sign-consistent | across ≥ 3 seeds |
| **Strong PASS** | U-H1..U-H5 AND delay128 degradation ≤ 20% AND distractor4x precision ≥ 0.70 AND `oracle_gap` ≤ 0.15 | all |

Results that meet only part of the gates are reported as PASS/FAIL per
gate — no silent aggregation.

## Run

Uses the repo `.venv` (torch 2.x, numpy, pyyaml, pytest, matplotlib).

```bash
# smoke (seconds): wiring check, not a quality signal
python -m pytest experiments/u0/tests
python -m experiments.u0.train --config experiments/u0/configs/smoke.yaml \
    --model gru64 --seed 0 --run-id smoke

# device benchmark (cpu vs mps, ~8 updates each)
python -m experiments.u0.bench_device --config experiments/u0/configs/smoke.yaml

# full matrix: 3 models x 3 seeds, then eval + causal + OOD + probes
caffeinate -ims python -m experiments.u0.sweep \
    --config experiments/u0/configs/default.yaml \
    --models mlp gru64 gru128 --seeds 0 1 2 --device cpu

# report + figures
python -m experiments.u0.report
```

Budget controls honored without code edits: `U0_TIME_BUDGET` (seconds),
`U0_MAX_ITERS`, `U0_NUM_ENVS`.

## Run artifacts

Per run under `experiments/u0/artifacts/runs/{model}_s{seed}/`:
`config.json` (env+train+git commit+device), `metrics.jsonl` (per
update), `best.pt` (lowest val `error_full`), `last.pt` (every
`checkpoint_every` updates), `done.json` (wall clock, device, threads,
commit). Sweep distinguishes completed (skip) / partial (resume) /
missing (start). Metrics are logged per update; only evaluation records
retain per-episode stats. Run artifacts are not committed.

## Claims boundary

A PASS supports: *in this finite-memory homeostatic environment, the
learned policy's storage decisions are conditioned on internal state in a
way that causally improves later regulation.* It does not support claims
about autobiographical memory, semantic understanding, or desire.
