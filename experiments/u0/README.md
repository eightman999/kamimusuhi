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
same event, same world, same history, same memory state
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

Four useful functions compete for **3 memory slots**, so covering every
function is impossible; the correct subset depends on which variables are
vulnerable. Need identities are **never labeled**: needs are
sampled per episode weighted by the reset-time deviation profile
(`need_beta` softmax + `need_floor`), so the only available signal is the
correlation between the agent's observed internal state and the identity
of the coming crisis.

## Memory is required — the protocol fix

After the event window, the current observation carries **no**
location↔function information. Post-onset the agent may know *which*
variable is failing (its internal channels) but never *where* the serving
site is. To make the memoryless search fail in principle:

- the ring has **12 locations** with only **8 sites**; a crisis resolves
  only at the potent site (a meager site gives partial relief, no
  resolution once the variable is deep) — the effective target is one
  location out of twelve;
- crisis drains **accelerate with age** (`crisis_accel`) — the
  recall→move→act chain fits the early window, but a wanderer trying
  sites one at a time runs out of time;
- **memory is smaller than the useful set**: 4 functions but only
  **3 slots**, so covering every function is impossible — which function
  to hold is the need-conditioned decision the experiment measures;
- each need onset applies a **shock** pushing the variable 15% toward its
  lethal bound, then keeps draining — every crisis is a ~8–12 step
  emergency, far shorter than a blind ring tour (~2 steps per location
  tried);
- all four variables can die (certainty collapse < 0.05 counts as death),
  so no need type can be waited out.

Sanity, verified before any training (`protocol_check.py`, P0 gates):
oracle resolves ≈ 0.99 of needs while the `no_memory` baseline resolves
≈ 0.30; erasing the oracle's memory at need onset collapses it to the
no_memory level.

## Environment

- 12-location ring (A–L). `MOVE` wanders +1 step while searching; once a
  RECALL produced a destination (`recall_valid`), MOVE becomes directed
  — it steps the short way around the ring, 2 locations per step, since
  locomotion toward a known destination is purposeful. Positions are
  observed as sin/cos so the encoding scales with the ring size.
- Internal state: `energy`, `temperature`, `risk`, `certainty` with
  preferred ranges energy [0.40, 0.80], temperature [0.35, 0.65],
  risk [0.00, 0.30], certainty [0.40, 1.00]. Death: energy < 0.05,
  temperature outside [0.05, 0.95], risk > 0.98, certainty < 0.05.
- Sites: each of the 4 functions (resource→energy, shelter→temperature,
  safe_zone→risk, obs_point→certainty) exists at 2 distinct locations —
  one `potent` (1.0) and one `meager` (0.35) site. `ACT` at a matching
  site during a crisis pulls the variable `potency × (mid − x)` toward
  the preferred midpoint and resolves the need **only if the variable
  lands back inside its range** — the meager pull usually cannot reach
  from a deep crisis state. `ACT` elsewhere costs energy and raises risk.
- Events: 8 functional reports (truthful type+location+potency) early in
  the episode, plus DISTRACTOR (typed junk) and NOISE items.
- Needs: 2 per episode; onset = last functional event + U[24,80], second
  need +U[8,24] later. At onset the variable is shocked 15% toward its
  lethal bound, then drains (energy 0.035, certainty 0.030,
  temperature 0.030, risk 0.080 per step) until resolved or death.
- Observation: 32 dims — internal(4) + vulnerability profile(4: the
  reset-time deviation the need plan was sampled from; constitutional
  knowledge the agent can always see) + stored potency per function(4:
  the agent's own memory contents — the critic must see "needed func
  held" for store->outcome credit to exist) + position sin/cos(2) +
  event type one-hot(7) + event location sin/cos(2) + event potency(1)
  + memory occupancy(1) + crisis flag(1) + recall readout valid(1) +
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
  to-be-needed functions — upper bound), `no_memory` (never stores or
  recalls — wanders and tries each location once; the U0 lower bound).

## Metrics

Primary (the U0 loss): `crisis_error_auc` — per-need homeostatic error
integrated from onset to resolution / death / episode end, averaged over
planned needs (unlived steps forfeited at the death error).

Also primary: `need_resolution` (resolved / planned), `time_to_resolution`
(censored mean), `survival_after_need` (fraction of planned needs
survived).

Secondary/context: `error_full` (whole-episode error integral),
`survival_fraction`, `stable_fraction`, `important_retention` (needed
site held at onset, over fired needs), `store_precision`, action counts.

## Causal tests

| id | manipulation | expectation if memory is causal |
|----|--------------|--------------------------------|
| U-H3 | `targeted_erase` / `donor_shuffle` in the store→onset window | crisis AUC ≥ 1.30× clean, or resolution drop ≥ 0.20 |
| U-H4 | `need_intervention`: pin internal state AND memory during the event window | mean \|ΔP(STORE)\| ≥ 0.20 |
| U-H4b | `targeted_mediation`: erase only need-serving items at onset | mediation AUC closes ≥ half the clean→no_memory gap |
| U-C4 | `event_perm`: permute observed type/location channels | breaks channel-position memorization |

`erase` (blanket clear) remains available for the protocol sanity gate
(P0-5) but is not part of the main battery.

## OOD conditions

`delay96`, `delay128`, `delay160` (episode length extended),
`distractor2x`, `distractor4x`, `need_mapping_shift` (function→variable
rotation: sites still heal, the association moves), `event_perm`,
`capacity2` (2 slots for 4 functions — harsher selection pressure).

## Pre-registered gates (recorded before seeing results)

Protocol sanity (P0, must ALL pass before any PPO — see
`reports/U0_PROTOCOL_CHECK.md`):

| gate | criterion | threshold |
|------|-----------|-----------|
| P0-1 | oracle need_resolution | ≥ 0.90 |
| P0-2 | no_memory need_resolution | < 0.40 |
| P0-3 | oracle crisis_error_auc | < no_memory |
| P0-4 | oracle vs fifo/random | not worse on resolution AND auc |
| P0-5 | oracle + memory erase | resolution drop ≥ 0.30 |

Main gates:

| gate | criterion | threshold |
|------|-----------|-----------|
| U-H1 | `need_resolution` AND `normalized_regret` on crisis_error_auc (0 = oracle, 1 = no_memory) | ≥ 0.80 AND ≤ 0.50 |
| U-H2 | `store_precision` AND `important_retention` | both ≥ 0.75 |
| U-H3 | targeted_erase OR donor_shuffle | auc ≥ 1.30× clean or resolution drop ≥ 0.20 |
| U-H4 | need-intervention mean \|ΔP(STORE)\| | ≥ 0.20 |
| U-H4b | targeted_mediation gap closed toward no_memory | ≥ 0.50 |
| U-H5 | learned > no_memory, erase hurts, probe Δ > 0 | sign-consistent across ≥ 3 seeds |
| **Strong PASS** | U-H1..U-H5 AND delay128 regret ≤ 0.70 AND distractor4x precision ≥ 0.70 AND need_mapping_shift resolution ≥ 0.50 AND capacity2 regret ≤ 0.50 | all |

Results that meet only part of the gates are reported as PASS/FAIL per
gate — no silent aggregation.

## Run

Uses the repo `.venv` (torch 2.x, numpy, pyyaml, pytest, matplotlib).
Apple Silicon settings (avoid oversubscription):
`OMP_NUM_THREADS=8 VECLIB_MAXIMUM_THREADS=8 OPENBLAS_NUM_THREADS=1`,
`torch_num_threads=8`.

```bash
# smoke (seconds): wiring check, not a quality signal
python -m pytest experiments/u0/tests
python -m experiments.u0.train --config experiments/u0/configs/smoke.yaml \
    --model gru64 --seed 0 --run-id smoke

# protocol sanity gate — MUST pass before any training
python -m experiments.u0.protocol_check --episodes 512

# device benchmark (cpu vs mps, ~100 updates each; adopts mps only if >=1.2x)
python -m experiments.u0.bench_device --config experiments/u0/configs/smoke.yaml

# full matrix: P0 gate -> 3 models x 3 seeds -> eval + causal + OOD + probes
# --device accepts cpu|mps|auto; auto always resolves to cpu (CPU-first)
# --scaffold adds the curriculum arm (C suffix); --teacher-iters adds the
# mechanics-only teacher arm (T suffix). Both are tagged and disclosed.
caffeinate -ims python -m experiments.u0.sweep \
    --config experiments/u0/configs/default.yaml \
    --models mlp gru64 gru128 --seeds 0 1 2 --device cpu \
    --scaffold

# report + figures
python -m experiments.u0.report
```

Budget controls honored without code edits: `U0_TIME_BUDGET` (seconds),
`U0_MAX_ITERS`, `U0_NUM_ENVS`.

## Run artifacts

Per run under `experiments/u0/artifacts/runs/{model}_s{seed}/`:
`config.yaml` (env+train+git commit+device), `metrics.jsonl` (per
update), `best.pt` (lowest val `crisis_error_auc`), `latest.pt` (every
`checkpoint_every` updates), `meta.json` (wall clock, device, threads,
commit; doubles as the completion marker). Sweep distinguishes
completed (skip) / partial (resume) / missing (start), and records any
failed stage to `artifacts/results/failures.json` instead of aborting.
Metrics are logged per update; only evaluation records retain
per-episode stats. Run artifacts are not committed.

Optional mechanics-only teacher bootstrapping (`train.imitation_iters`
> 0) lets an oracle demonstrate the memory API / recall→move→act
sequence; steps where the oracle stores are masked out of the BC loss,
so need relevance is never taught. Teacher runs are tagged in
`config.yaml` and reported separately from U0-main.

**Learning note (honest).** Pure PPO in this environment reliably
learns *crisis-response by search* (wander + ACT) but stalls on the
memory pathway: a memoryless policy resolves ~25–35% of needs, which is
a strong local optimum — RECALL and the right STORE are never sampled
enough to discover the chain. The `--scaffold` arm adds a disclosed
training curriculum that breaks the deadlock *without* labeling which
events matter:

- `crisis_mask_iters`: during an active crisis, idle actions are masked
  so the sampled choice set is {STORE, RECALL, MOVE, ACT} — RECALL gets
  sampled and visibly works when memory holds the item.
- `functional_store_iters`: STORE is only available on functional events
  early on, so the student's memory always holds real sites (junk stores
  otherwise evict the needed item and hide the recall payoff).
- `delay_curriculum_start/iters`: needs fire ~8 steps after the event
  window at first (tight store→outcome credit), annealing to the
  configured delay.

All evaluation/validation still runs the un-scaffolded target task. The
agent still discovers *which* function to store from reward — the masks
only prune provably-irrelevant choices. Whether pure PPO or the scaffold
arm is needed is itself a reported result (U-H5 asks for the effect
across seeds).

## Claims boundary

A PASS supports: *in this finite-memory homeostatic environment, the
learned policy's storage decisions are conditioned on internal state in a
way that causally improves later regulation.* It does not support claims
about autobiographical memory, semantic understanding, or desire.
