# U0 compact results — evidence snapshot for catalog provenance

This file is an **evidence snapshot**: it persists, in tracked form, the
minimal evidence that the `u0-need-guided-memory` entry of
`knowledge/experiment-findings.json` relies on. It is **not** the official
U0 all-gate summary report (none exists yet) and asserts no formal gate
verdict beyond the narrow comparison below. The protocol sanity PASS is
evidenced separately by `experiments/u0/reports/U0_PROTOCOL_CHECK.md`,
which this snapshot depends on for that claim.

## Source material

- Origin file (untracked, intentionally gitignored):
  `experiments/u0/artifacts/results/summary.csv`
- Origin SHA-256: `20a5f7f4fd1e9a83fdab738aab1417dfa5b9203dfc661fa6e6b87f8de614fef8`
- Shape: 300 data rows + 1 header line (301 lines total)

## Run metadata (from `artifacts/runs/*/meta.json` and `results/failures.json`)

- Sweep config: `experiments/u0/configs/default.yaml` (`name: u0_default`)
- Trained subjects: mlp / gru64 / gru128 x seeds 0-2, pure-PPO arm and
  disclosed scaffold arm (`C` suffix) — 18 learned runs, 7 baselines
- Training: 800 iters per run, device=cpu, torch_num_threads=8
- Training commits: `b198005a` (pure-PPO arm), `bf32414a` (scaffold arm)
- Evaluation: 256 episodes per condition, eval seed 900001
  (`sweep.py` defaults; protocol sanity used seed 900001 / 512 episodes)
- `results/failures.json`: [] (no failed stages)

## Condition definition used by the catalog claim

"learned clean" = rows whose `subject` is a trained run (any subject not
starting with `baseline_`; both pure-PPO and scaffold `C` arms) AND
`causal == "none"` AND `ood == "none"`. 18 rows match.
(The same maxima result if the scaffold arm is excluded: the argmax row
is in the pure-PPO arm either way.)

## Recomputed values (learned clean)

| metric | max | argmax row |
|---|---|---|
| need_resolution | 0.60546875 | gru128_s0 |
| store_precision | 0.530078125 | gru128_s0 |

## Pre-registered thresholds (`experiments/u0/README.md`)

- U-H1: `need_resolution` >= 0.80 (and normalized regret <= 0.50)
- U-H2: `store_precision` >= 0.75 (and `important_retention` >= 0.75)

## Limited conclusion

0.60546875 < 0.80 and 0.530078125 < 0.75, so on the learned-clean subset the
U-H1 and U-H2 thresholds are **not met** — the existing catalog claim
(need_resolution max ~0.6055, store_precision max ~0.5301; U-H1=0.80,
U-H2=0.75; therefore need-guided memory acquisition cannot be counted as
successful) is reproduced. Nothing about other gates (U-H3..U-H5, strong
PASS conditions) is asserted here.

## Method

Values were recomputed from `summary.csv` rather than hand-copied:

    rows = csv.DictReader(summary.csv)
    clean = [r for r in rows
             if not r['subject'].startswith('baseline_')
             and r['causal'] == 'none' and r['ood'] == 'none']
    max(float(r['need_resolution']) for r in clean)   # -> 0.60546875
    max(float(r['store_precision']) for r in clean)   # -> 0.530078125

Cross-check: the argmax row equals the per-condition eval artifact
`results/gru128_s0_c-none_o-none.json` (`metrics.need_resolution`,
`metrics.store_precision`) byte-for-byte in float terms. Each summary row
aggregates 256 evaluation episodes at eval seed 900001, produced by
`experiments/u0/sweep.py` phase 2.
