# Kamimusuhi G0-v6 — CPC Grounding Replication

G0-v6 tests whether the CPC signal observed in the G0-v5 seed-0 pilot
replicates across five seeds, including the apparent split between
context/composition matching and raw future-observation readout.

The current locked run is revision 2 (`G0-v6.1`), a corrective rerun after a
revision-1 evaluator audit found an unseen-horizon negative-scoring bug. The
revision-1 outputs are preserved separately and excluded from the final
aggregate.

The experiment is self-contained under this directory. Existing
`experiments/g0`, `experiments/g0_v4`, and `experiments/g0_v5` are read-only
references and are not overwritten. The copied frozen dataset is checked
against the G0-v5 dataset manifest; no old checkpoint or result is read.

## Fixed protocol

- CPC InfoNCE, observation sequence only, horizons `[1, 4]`, 20 epochs.
- Seeds `0..4`, with seed 0 treated as a full-test seed, not a pilot.
- Initial checkpoints are saved before optimization and used as exact twins.
- Primary health gate: `iid/train_like/recall_at_1` must be strictly higher
  for trained CPC than its exact twin.
- Every epoch records finite loss, latent mean/std, per-dimension std,
  covariance spectrum, effective rank, pairwise cosine distribution, latent
  norm, perturbation sensitivity, and label-free temporal retrieval.
- Effective rank is diagnostic only; it is not an automatic stop condition.
- OOD labels are evaluator-only and never select checkpoints or parameters.

## Execution

Use the repository root and a Python with `torch`, `numpy`, `PyYAML`,
`matplotlib`, and `pytest`:

```sh
python -m pytest experiments/g0_v6/tests -q
python -m experiments.g0_v6.src.run audit
python -m experiments.g0_v6.src.run protocol-lock
python -m experiments.g0_v6.src.run controls
python -m experiments.g0_v6.src.run smoke --device cuda:0
```

The full GPU schedule is fixed in `scripts/run_full.sh`:

```sh
bash experiments/g0_v6/scripts/run_full.sh
```

It runs seeds 0, 2, 4 sequentially on CUDA 0 and seeds 1, 3 sequentially
on CUDA 1, then evaluates, aggregates, creates plots, and writes
`G0_V6_REPORT.md`. Existing completed outputs are only reused when their
protocol identity matches; mismatched or partial outputs fail closed.

## Metrics

The report contains raw values and trained-minus-twin deltas for IID,
mid-context stability, match OOD, dynamic-segment OOD, composition OOD,
intervention distances/selectivity, temporal retrieval, and future-observation
ridge MSE. Retrieval is split into IID, context-OOD, and altered-segment
timing, with train-like and unseen horizons. PCA geometry plots are
descriptive only.

The final verdict is one of `G0-V6_STRONG_PASS`, `G0-V6_PARTIAL`,
`G0-V6_FAIL`, or `G0-V6_INVALID`, with the exact seed-direction counts shown
in the report.
