# CX0 — Mio Synthetic Cortex / Cortical Mantle

Status: **PARTIAL** — overnight matrix complete (v3, 10 seeds; see
`reports/CX0_OVERNIGHT_REPORT.md`). C-G2 failed: no cortex advantage over
the parameter-matched GRU at this scale. Kernel hardened post-review:
perception is rolled once per step (t-cached), so obs/STORE/oracle see
one event stream; runs_v2/runs_v3 were generated under the pre-fix kernel.
Series: Phase F — integration of previously validated cognitive organs
Depends on verdicts: H0 PASS · R0 PASS · S0 PASS · T0 PASS · O0 PARTIAL (shadow) · P0/G0 FAIL (excluded)

## Scientific question

> Do previously validated low-level cognitive mechanisms, composed through a
> recurrent cortical-like integration layer, produce longer-timescale
> context-dependent behavior that the organs alone cannot?

Explicitly **not** under test: whether a bigger/better NN helps. The control
architecture (C1) is a parameter-matched generic GRU over identical inputs.

## Admission policy (canonical)

The promotion manifest of PR #39
(`docs/implementation/experimental-organ-integration.md` on
`feat/integrate-experimental-organs`) is adopted as the canonical admission
policy. Summary, also recorded in `docs/research/history.md` §5:

- **Active organs**: H0 (Regulation), R0 (MemoryGate), S0 (AgencyAttribution),
  T0 (TemporalState).
- **Shadow only**: O0 (ObjectState). Its signal may be logged/compared but is
  never a required input; any use is flagged `shadow_derived` in logs.
- **Excluded**: P0 (predictive surprise), G0 (emergent grounding). Their
  outputs are not wired in as validated signals. Hidden units of the cortex
  receive **no semantic labels** — grounding remains a separate open problem.
- The cortex output is an organ-style derived signal: not canonical evidence,
  not durable memory, not self-state, not mutation authority.

## Architecture arms

All arms see the **identical** `OrganSignalBundle` and the same frozen organ
checkpoints. Only the integration layer differs.

| Arm | Integration layer | Role |
|---|---|---|
| **C0** | flat MLP over instantaneous signals | baseline: organs only, no cross-organ recurrence |
| **C1** | single generic GRU, parameter-matched to C3 | "just add an NN" control |
| **C2** | layered recurrent stack (assoc → context → prediction → feedback) | structured-dense cortex |
| **C3** | modular sparse cortex: distinct populations, sparse local + sparse long-range connectivity, heterogeneous time constants | **Mio Cortical Mantle v0** — the hypothesis |

Organ internals (T0/S0 recurrence, R0 slot memory) are identical in all arms
including C0 — what differs is whether a *cross-organ* recurrent state exists.

## Organ signal schema (CX0 input contract)

`OrganSignalBundle` — the fixed input vector for every arm:

| Field | Dims | Source organ | Content |
|---|---|---|---|
| `sensory` | 16 | env | current observation (self-coupled + world + event/cue channels) |
| `h0` | 9 | H0 organ | internal vars (energy, fatigue, temperature, novelty, certainty) + urgency + emergency flag + drive embedding (2) |
| `s0` | 8 | S0 organ | self-prob, world-prob, agency confidence, prediction-error summary, attribution moments |
| `t0` | 18 | T0 organ | elapsed estimate + decoded delay + temporal confidence + due channel + embedding (14) |
| `r0` | 13 | R0 organ | retrieved payload (8) + availability flag + direction pointer + slot occupancy summary (3) |
| `o0` | 5 | O0 shadow (optional, default off) | hidden-state position estimate + confidence |

Action side: a fixed 11-action space shared by all tasks
(`NOOP, FWD, BACK, INTERACT, REST, STORE, RECALL, RESP0..3`).
`STORE`/`RECALL` are the cortex→R0 admission/retrieval requests.

## Two-stage protocol

- **Stage A — organ pretraining (shared).** Each organ module is trained on
  its own validated subtask instantiated inside the CTX world, then frozen.
  Checkpoints are per-organ and identical across arms and seeds.
- **Stage B — integration training.** Only the integration layer + sensory
  encoder + policy head are trained, by behavior cloning on oracle
  trajectories (DAGGER-style mixing where needed), then optionally PPO.
  BC is the primary, controlled comparison; any PPO stage is reported
  separately.

## Tasks — CTX world

One shared `CtxWorld` kernel (ring of locations, internal homeostatic
variables, action-coupled vs exogenous sensory dims, discrete events, cue
channel). Task wrappers fix reward/termination/oracle:

| Task | Required organs | Structure |
|---|---|---|
| **CTX-1** | H0+R0 | hunger drive → must have stored the food-site event earlier → navigate to remembered site |
| **CTX-2** | S0+R0 | distinguish self-caused stimulus change from exogenous event; only world-events carry the cue that must be remembered and acted on |
| **CTX-3** | T0+R0 | cue at t=0 selects a response; execute it after the encoded delay |
| **CTX-4** | H0+T0+R0 | optimal action = f(body state × elapsed time × remembered context) — no pair suffices |
| **CTX-5** | all | conflicting drives (rest vs remembered reward vs risk); graded arbitration measured against oracle preference ordering |

Every task ships a scripted `oracle_policy` — both the BC teacher and the
"simple heuristic" control required by the failure criteria.

## Controls and interventions (pre-registered)

- **Input shuffles**: H0 / S0 / T0 / memory bundle / cortex hidden state,
  each shuffled independently across episodes.
- **Cortex OFF**: integration output zeroed; policy head sees bypass path.
- **Lesion matrix** per task: −H0, −R0, −S0, −T0, −Cortex, −context
  population, −feedback population (C3 only where applicable).
- **Hidden-state probes**: linear decode of the task's context variable from
  each population (C-G6).
- **Parameter accounting**: all arms report parameter counts; C1 is sized to
  match C3 within ±10%.

## Pre-registered gates

Pass criteria (from the research plan, verbatim in intent):

- **C-G1** C3 (or winner) beats C0 on multi-organ task performance.
- **C-G2** improvement over parameter-matched generic GRU on at least part of
  the composite tasks.
- **C-G3** task-specific degradation under individual organ shuffles.
- **C-G4** cortex hidden reset degrades long-context tasks.
- **C-G5** no large regression on single-organ tasks from adding the cortex.
- **C-G6** a persistent, linearly decodable context representation exists in
  at least one hidden population.

Failure conditions (any ⇒ FAIL recorded, not deleted):

- no difference vs C1; performance invariant to Cortex OFF or organ shuffle;
- a simple heuristic beats all arms on every task;
- cortex merely re-implements one organ's function;
- hidden state fully explained by current observation;
- train-scenario memorization;
- effects explained by parameter count alone.

5 seeds per arm; seeds are the replication units (as in R0/T0).

## Layout

```text
env/ctx_world.py   shared world kernel + CTX-1..5 task wrappers + oracles
organs/            organ modules + frozen pretraining wrappers + bundle schema
models/            c0_flat / c1_gru / c2_layered / c3_mantle + shared heads
memory/            slot memory (R0 mechanics, adapted from experiments/r0)
train_bc.py        Stage-A pretraining + Stage-B imitation
evaluate.py        success, shuffles, lesions, probes, param counts
analysis/report.py report generation → reports/CX0_RESULTS.md
configs/           default.yaml / smoke.yaml
tests/             env, organ, model, determinism tests
```
