# G0-v6 lineage

## G0-v5 → G0-v6

G0-v5 was an independent observation-only predictive-invariant grounding
screen. Its predeclared seed-0 pilot admission produced `G0-V5_FAIL`; that
decision is not changed or re-evaluated here. No G0-v5 checkpoint, result,
pilot gate, or failure classification is used as a training or evaluation
result for G0-v6.

G0-v6 uses the G0-v5 frozen synthetic dataset and the equivalent fixed
distance-evaluation contract as a read-only protocol reference. The copied
dataset is byte-checked against the G0-v5 frozen manifest. The CPC model is
initialized afresh for each seed. G0-v5 source/data hashes and the audit
outcome are recorded in `manifests/g0_v5_reference.json` and
`manifests/equivalence_audit.json`.

## G0-v6

G0-v6 is an independent replication experiment:

- CPC only, with a fixed `[1, 4]` InfoNCE horizon configuration.
- Five prespecified seeds: 0, 1, 2, 3, 4.
- Exact untrained twins use each run's saved initial `state_dict`.
- Raw and train-only PCA controls use the same frozen evaluation split.
- The primary health metric is label-free temporal retrieval
  `iid/train_like/recall_at_1`; no cause/context labels enter training,
  checkpoint choice, or admission.
- Low effective rank is recorded as a diagnostic and does not by itself stop
  a run. Only full collapse, numerical failure, leakage, system failure, or
  an invalid evaluation identity stops or invalidates a run.

The G0-v5 FAIL result remains historical and unchanged.

## G0-v6.1 corrective revision

The first G0-v6 execution (revision 1) was stopped from being accepted after
post-evaluation audit found that the analytic unseen-horizon retrieval helper
used a target-to-target score for non-positive candidates. Its outputs are
preserved as an invalid-revision archive and are not included in the revision
2 aggregate. The required fix is evaluator-only; nevertheless, the fixed
revision reruns all five trainings and all paired evaluations with a new
config/protocol lock. A PCA plotting reshape bug was corrected at the same
time. No result is selected or discarded using OOD values.
