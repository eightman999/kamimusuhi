# M1 Preflight Runbook

M1 §22 forbids fixing the run's parameters in advance. Population size,
generation count, device configuration, cheap evaluator, mutation pressure and
disturbance schedule are all decided **from measurement on the actual
hardware**, and the M1 run does not start until every step below has produced a
number.

Target host: `llm-machine` (hostname `master`) — RTX 3060 12GB + Tesla P100
16GB, CUDA 12.6, venv `~/mioba-venv` from `requirements-gpu-cu126.txt`.
Nothing in this document has been run by the authors: the development machine
is CPU-only, so every GPU figure here is a placeholder to be filled in.

```bash
cd ~/kamimusuhi && export MIOBA_PYTHON=~/mioba-venv/bin/python
export PYTHONPATH=~/kamimusuhi
mkdir -p m1-preflight
```

## 0. Identity

Record what is being measured, before measuring it.

```bash
$MIOBA_PYTHON -m experiments.mioba.cli env-info | tee m1-preflight/env.json
git rev-parse HEAD
$MIOBA_PYTHON -c "
from experiments.mioba.cli import load_config
from experiments.mioba.genome.hashing import config_hash, scientific_config_hash, runtime_config_hash
c = load_config('experiments/mioba/configs/m1_pilot.yaml')
print('config     ', config_hash(c))
print('scientific ', scientific_config_hash(c))
print('runtime    ', runtime_config_hash(c))"
```

The scientific hash now includes the simulator's own semantics
(`simulator_semantics_version` 2, `rng_protocol_version` 2,
`propagation_backend`), so it will differ from M0's even where the YAML looks
similar. That is correct: M1 is not the same experiment.

**M0 recordings cannot be replayed by this build.** The base graph is sampled
from `fba.base_seed` instead of the genome seed (dataset identity `v0-…` →
`v1-…-s<seed>`), the delay line was off by two steps, and the Poisson drive
consumes a different random stream. Replay refuses such rows explicitly rather
than re-running them on a different network; use the M0 commit for M0 data.

## 1. Evaluation profile and slot count (§2, §2.2)

```bash
for DEV in cpu cuda:0 cuda:1; do
  $MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/m1_pilot.yaml \
    profile --device $DEV --evaluations 6 --slots 1,2,3 \
    --out m1-preflight/profile_${DEV/:/}.json
done
```

Read off, per device:

- the phase breakdown (`topology_construction` must be ~0 after the first
  evaluation — the base graph is resident; if it is not, the cache is missing);
- `successful_evaluations_per_minute` per slot count, and the selected slots.
  This is the decision criterion, **not** per-job latency: three concurrent
  evaluations that are each 40% slower are a win.
- OOM at a slot count is a result, not a failure. It is recorded and the sweep
  continues.

## 2. Where event-driven propagation stops paying (§2.4-4)

```bash
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/m1_pilot.yaml \
  profile --device cuda:0 --evaluations 2 --slots 1 --activity-sweep \
  --out m1-preflight/activity_cuda0.json
```

If `break_even_active_edge_ratio` comes back non-null, set
`fba.dense_above` to it and `fba.propagation_backend: auto`. If it is null on
both GPUs (as on CPU at 20k/200k, where the event path still won 4.8x at a
21.9 Hz population rate), leave `event_csc` and rely on the
`high_activity_individual` warning to catch a hyperactive mutant.

## 3. Device choice (§20)

```bash
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/m1_pilot.yaml \
  device-bench --devices cpu,cuda:0,cuda:1 --slots 1,2,3 --evaluations 8 \
  --out m1-preflight/device_bench.json
```

Exit code 7 means the devices disagreed on the resulting firing rates — stop
and investigate; one of them is not running the experiment that was asked for.

Using a GPU is not the goal. Event-driven propagation replaced a
bandwidth-bound matmul with gathering the edges of ~130 spiking neurons, which
is small, latency-sensitive work; the CPU may win, and if it does, the run uses
it. Record evaluations/minute, p50/p95 latency, peak VRAM and active-edge
density per device and pick on throughput.

## 4. Mutation sensitivity (§10)

```bash
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/m1_pilot.yaml \
  sensitivity --device <fastest-from-step-3> --out m1-preflight/sensitivity.json
```

Paste the printed `parameter_weights` and `parameter_scale_by_path` into
`evolution.mutation` in the run config. A parameter that moves nothing is drawn
*less often* and given a wider range — never deleted, which would be a claim
about the model rather than a measurement of it.

On a small CPU network this already reproduces the M0 diagnosis: `vThr`
dominates and `wScale` / `tauMem` / `tauSyn` move nothing measurable. Whether
that holds at 139k/14M is exactly what this step answers.

## 5. Cheap evaluator (§2.3, §21)

Gold is M0's condition: 500 ms x 8 replicates.

```bash
$MIOBA_PYTHON -m experiments.mioba.cli --config experiments/mioba/configs/m1_pilot.yaml \
  rank-check --device <fastest> --population 24 \
  --gold-duration-ms 500 --gold-replicates 8 \
  --candidates 250x2,500x2,250x4,500x4 \
  --out m1-preflight/rank_check.json
```

Exit 0 = a candidate was adopted; exit 6 = none ranked like gold, and
generations are updated with the gold evaluator. The search takes the
**cheapest** candidate that passes rho >= 0.85 and top-8 overlap >= 6/8; it does
not take the best-scoring one, which would buy agreement with compute.

Ranking is on `selection_score`, not the M0 placeholder: a cheap evaluator that
preserves the placeholder's order while scrambling the disturbance response is
not usable for M1.

## 6. Derive the run parameters

From steps 1-5:

| quantity | comes from |
|---|---|
| device + `worker.slots` | step 3 `recommended_device` / `recommended_slots` |
| `worker.execution_batch` | step 1 (still 1 unless the bench says otherwise) |
| `evaluation.duration_ms` / `replicates` | step 5 adopted candidate |
| `evolution.mutation.parameter_*` | step 4 recommendation |
| `fba.propagation_backend` / `dense_above` | step 2 |
| `population.target_size` | see below |
| `evolution.max_generations` | see below |

Estimated generations per hour:

```
generations/hour = 60 x evaluations_per_minute_total / population.target_size
```

where `evaluations_per_minute_total` is the sum over the devices actually used.
Pick `population.target_size` and `max_generations` so the run fits the 60
minute cap from M1 §20 with margin, and write both numbers *and the measurement
they came from* into the run's config comments.

## 7. Pre-run checks

```bash
# both GPUs idle, services down
nvidia-smi; systemctl is-active llama-master open-webui
# disk and RAM headroom for the runs dir
df -h ~/mioba-runs; free -g
# the suite passes on the run host, with torch present
$MIOBA_PYTHON -m pytest experiments/mioba/tests -q
# back up before starting
mkdir -p ~/mioba-backups
```

Then start the coordinator, the workers (`--slots` from step 3), and open the
Live Observatory. The top page shows the population; GPU cards moved to
Infrastructure.

## 8. What to watch during the run

| signal | where | what it means |
|---|---|---|
| `runtime_resource_retry` | event feed | a worker narrowed its concurrency after an OOM; the job was re-queued with its seed. Not a death, not a fitness penalty. |
| `high_activity_individual` | event feed | an organism is activating more than `activity_warn_edge_ratio` of the connectome per step and is slowing its worker. Check `fba.dense_above`. |
| `invalid_structure` / `neutral_structure` | event feed | a prune orphaned an organ, or a circuit is not on an FBA0→FBA0 path. Expected; watch the ratio. |
| neutral + invalid organ counts | Live Observatory | if these dominate, structural mutation is producing dead tissue faster than selection removes it. |
| `scientific_config_mismatch` | events | the config changed under a running experiment id. Stop. |

## 9. Which individuals get the gold evaluator

Generations are updated with the cheap evaluator. Re-run on gold:

- each generation's champion by `selection_score`;
- the final champion;
- every individual flagged as a notable organism;
- every genome whose birth carried a `NEW_ORGAN` or `DUPLICATE_ORGAN` that
  survived to the next generation;
- anything anomalous (terminated episodes, `high_activity_individual`).

## 10. Ablation (M1 success condition)

For the champion, re-evaluate with its artificial organs disabled
(`DISABLE_ORGAN` on each, or a genome variant with `enabled: false`) under the
**same** evaluation seed, and compare `task_score`, `selection_score`, activity
and disturbance recovery. A reproducible difference in at least one is the
evidence that the grown circuit does something.
