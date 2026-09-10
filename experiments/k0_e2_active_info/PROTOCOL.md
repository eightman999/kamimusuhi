# K0-E2 protocol, fixed before full runs

K0-E is immutable; `baseline_manifest.json` records its existing local files.
No push, external publication, persistent service enablement, or baseline rewrite.

## Independent units and selection

MLP, GRU64 and GRU128 each use seeds 0–7. Eight seeds are the independent
replicates; episode counts are never substituted for statistical sample size.
Imitation environment streams start at 100000+seed, PPO at 200000+seed,
critic warm-up at 300000+seed, validation at 700001,
held-out evaluation at 900001. Evaluation RNG streams are common across models
for paired comparisons but disjoint from training. No test-based tuning.

Stage A primary: 256 imitation updates, 1024 environments, 48 transitions/episode,
Adam learning rate .001, full-sequence training. Demonstrations use the
information-constrained analytic teacher. Updates 1–128 follow the teacher;
129–256 use 80% teacher / 20% sampled learner actions per environment step
with teacher labels on the resulting states (DAgger exposure).
Action-balanced CE is additionally weighted at decisions (32x) and
acquisition actions (4x). No hidden truth is a policy input.
Training cost/reliability/latency are sampled from the specified grids.
Fixed validation evaluates 256 episodes at cost .05, reliability 1, latency 2.
Checkpoint criterion: scenario macro success + .1 * mean episode reward.
All architecture comparisons use imitation-best; final weights are also scored.

Stage B: GRU128, all eight paired seeds, from identical imitation-best
model and optimizer checkpoints, 8 PPO updates, 1024 environments, 2 epochs,
256-environment minibatches. Adam state and rollout RNG start identically.
Full recurrent sequences are maintained in each minibatch.

| Arm | Single change from B1 |
|---|---|
| B0 | Imitation only |
| B1 | Existing PPO: lr .001, gamma .99, lambda .95, ratio clip .2, value .5, entropy .01, global normalized advantage, gradient clip 1.0 |
| B2 | Training language call cost zero; evaluation cost unchanged |
| B3 | Learning rate .0001 |
| B4 | Four critic-head-only warm-up updates; shared core and actor fixed |
| B5 | Value error divided by rollout return SD (advantage normalization already exists in B1) |
| B6 | Add .1 KL(new policy || frozen imitation policy) |
| B7 | Gradient clipping threshold .25 (B1 already clips at 1.0) |

PPO's existing clip and advantage normalization mean that B5/B7 test *additional
scaling/stronger clipping*, not their presence versus absence. B4 warm-up consumes
its own temporary stream, then restores the common rollout/global RNG state.
Only validation-improving factors may form one finite combined candidate;
improvement is not assumed and failure is an admissible scientific result.

## Evaluation

One-factor cost (9: the specified seven plus .8 and 1.2 to cross the
analytic +/-1 reward threshold), reliability (4), latency (6) sweeps; six gate interventions;
nine response conditions; paired CALL/NO CALL forks; ORIENT/OBSERVE interventions;
eight retention delays through 640 and five hidden-state conditions; fourteen
sensor/OOD conditions; finite habituation and shortcut audits.
Default 256 evaluation episodes; long retention uses 64 episodes per condition.
All results identify their actual episode count and seed.
Matched random gate controls must preserve actual accepted call counts, not just
use an unverified Bernoulli probability.

Seed-level mean, SD, median and bootstrap 95% CI; paired exact sign tests.
No automatic seed expansion is justified solely to seek significance. Sixteen
seeds require a scientific ambiguity that would change the primary conclusion.

J72 runs only after scripted closed-loop tests pass. Representatives are selected
within each GRU architecture using validation alone. HTTP, parse, semantic and
downstream success are separate measurements. Restore the initial stopped state.

## Completion and interpretation

Protocol execution and research success are distinct. Preserve failed runs,
NaN checks, checkpoint stages/RNG/source hashes and actual GPU UUIDs.
Report unmet gates, no-language Bayes ceiling and synthetic-task limitations.
Only the final response asks whether to proceed to K1.

## Validation-only finite escalation

The initial 64-update, 24-run attempt is preserved under `artifacts/runs`.
Before any held-out evaluation, initial validation showed GRU call rate zero
and success around .57, versus MLP around .73 with calls. This underfitting
justifies one common increase to 256 fresh updates for all three architectures
and eight seeds, isolated under `artifacts/primary`. No further training-budget
search is authorized by this protocol. Primary PPO branches use these primary
imitation-best checkpoints. Initial and primary attempts are never pooled as
independent seeds.

The same seed always uses the same physical GPU (even: RTX 3060; odd: P100),
including every paired PPO arm. B4 records additional critic warm-up compute.

Combined-candidate rule (fixed before reading ablation results): choose factors
B3–B7 whose mean final validation-score gain over B1 exceeds .01 with paired
two-sided exact sign p<.05. B2 changes the objective and is causal-only. If fewer
than two factors qualify, do not duplicate a single-factor run. Otherwise run one
B8 combination on the same eight parent checkpoints. Factor-composition tests
require bit-exact equality with the corresponding existing single-factor update.
