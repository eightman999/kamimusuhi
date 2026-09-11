# R0 — Learned Memory Gate: Results

Eval corpus: seed 900001+, 256 episodes/condition. Cells are mean±std across seeds (seeds are the replication units). Baselines are single deterministic policies (std=0).

## Clean task accuracy

| subject | accuracy | imp. retention | store precision | mem. efficiency | stores | recalls |
|---|---|---|---|---|---|---|
| gru128 | 0.988±0.006 | 0.991±0.004 | 0.967±0.008 | 0.975±0.009 | 4.147±0.040 | 4.001±0.002 |
| gru64 | 0.960±0.009 | 0.966±0.006 | 0.889±0.086 | 0.919±0.056 | 4.559±0.555 | 5.822±2.576 |
| mlp | 0.999±0.001 | 1.000±0.000 | 1.000±0.000 | 1.000±0.000 | 4.000±0.000 | 4.000±0.000 |
| baseline/fifo | 0.533±0.000 | 0.440±0.000 | 0.253±0.000 | 0.315±0.000 | 15.504±0.000 | 4.000±0.000 |
| baseline/lru | 0.483±0.000 | 0.402±0.000 | 0.253±0.000 | 0.308±0.000 | 15.504±0.000 | 4.000±0.000 |
| baseline/oracle | 1.000±0.000 | 1.000±0.000 | 1.000±0.000 | 1.000±0.000 | 4.000±0.000 | 4.000±0.000 |
| baseline/random | 0.187±0.000 | 0.084±0.000 | 0.063±0.000 | 0.076±0.000 | 16.969±0.000 | 4.000±0.000 |
| baseline/reservoir | 0.507±0.000 | 0.450±0.000 | 0.293±0.000 | 0.364±0.000 | 9.195±0.000 | 4.000±0.000 |

## Causal tests (accuracy)

| subject | clean | R-C2 erase | R-C1 shuffle | R-C3 permute | R-C4 noise |
|---|---|---|---|---|---|
| gru128 | 0.988±0.006 | 0.134±0.011 | 0.117±0.004 | 0.991±0.006 | 0.992±0.004 |
| gru64 | 0.960±0.009 | 0.139±0.012 | 0.121±0.005 | 0.959±0.009 | 0.957±0.010 |
| mlp | 0.999±0.001 | 0.126±0.008 | 0.118±0.003 | 0.998±0.001 | 1.000±0.000 |
| baseline/fifo | 0.533±0.000 | 0.132±0.000 | 0.118±0.000 | 0.533±0.000 | 0.272±0.000 |
| baseline/lru | 0.483±0.000 | 0.132±0.000 | 0.118±0.000 | 0.483±0.000 | 0.268±0.000 |
| baseline/oracle | 1.000±0.000 | 0.132±0.000 | 0.113±0.000 | 1.000±0.000 | 1.000±0.000 |
| baseline/random | 0.187±0.000 | 0.105±0.000 | 0.134±0.000 | 0.187±0.000 | 0.143±0.000 |
| baseline/reservoir | 0.507±0.000 | 0.120±0.000 | 0.116±0.000 | 0.498±0.000 | 0.365±0.000 |

## OOD (accuracy)

| subject | clean | delay96 | delay128 | highnoise | slots2 |
|---|---|---|---|---|---|
| gru128 | 0.988±0.006 | 0.991±0.005 | 0.994±0.003 | 0.992±0.004 | 0.743±0.014 |
| gru64 | 0.960±0.009 | 0.962±0.023 | 0.967±0.022 | 0.957±0.010 | 0.697±0.027 |
| mlp | 0.999±0.001 | 0.999±0.001 | 1.000±0.000 | 1.000±0.000 | 0.753±0.004 |
| baseline/fifo | 0.533±0.000 | 0.205±0.000 | 0.153±0.000 | 0.272±0.000 | 0.208±0.000 |
| baseline/lru | 0.483±0.000 | 0.248±0.000 | 0.203±0.000 | 0.268±0.000 | 0.210±0.000 |
| baseline/oracle | 1.000±0.000 | 1.000±0.000 | 1.000±0.000 | 1.000±0.000 | 0.760±0.000 |
| baseline/random | 0.187±0.000 | 0.148±0.000 | 0.138±0.000 | 0.143±0.000 | 0.153±0.000 |
| baseline/reservoir | 0.507±0.000 | 0.449±0.000 | 0.409±0.000 | 0.379±0.000 | 0.333±0.000 |

## Verdicts

- [x] **PASS-1 learned > Random/FIFO** — learned 0.960 vs fifo 0.533
- [x] **PASS-2 memory-erase collapses accuracy** — erase 0.139 (chance 0.125)
- [x] **PASS-3 important retention > irrelevant retention** — imp_ret 0.966 precision 0.889
- [x] **PASS-4 high-noise robustness** — R-C4 noise 0.957
- [x] **PASS-5 3-seed replication (std<0.05)** — max seed std 0.009

Strong PASS:
- [x] **oracle gap** — best=mlp 0.999 vs oracle 1.000
- [x] **unseen delay transfer** — delay128 0.967
- [x] **halved capacity near oracle** — slots2 learned 0.697 vs oracle 0.760

Overall: **PASS (strong)**

_Provenance: policies were bootstrapped by DAGGER-style oracle BC (class-balanced, teacher-mixed rollouts) for the first 60 updates, then trained by PPO on task reward only. RECALL is causally required (R-C2 erase drops to chance) and readout is content-based (R-C3 permute is a no-op)._
