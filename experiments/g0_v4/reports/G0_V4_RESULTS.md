# G0-v4 — Predictive Invariant Grounding: Results
**Research question:** is G0-v3's FAIL explained by a weak reconstruction / one-step prediction objective, or does this setting resist self-supervised latent abstraction in general? v4 tests CPC / VICReg-temporal / JEPA-style objectives against a GRU-predictor control, each paired with an untrained twin.
**Final verdict: FAIL**
## Protocol integrity
- protocol_validator: **PASS** (config `experiments/g0_v4/configs/default.yaml`)
- label firewall: training sees only `obs/next_obs/actions` (TRAIN_KEYS whitelist; `eval_only_keys_stripped` in each meta.json).

## Methods
| method | objective | latent |
|---|---|---|
| gru | next-obs delta MSE (G0 control) | GRU hidden |
| cpc | InfoNCE: W_k c_t -> z_{t+k}, in-batch negatives | GRU context c_t |
| vicreg | two augmented views: invariance+variance+covariance + latent fwd prediction | GRU hidden h_t |
| jepa | action-conditioned latent rollout vs EMA target encoder (t+1..t+K) + var/cov guard | GRU context c_t |

All models additionally carry a small linear latent->obs readout (`v4.readout_weight`, default 0.1) solely so the G0 `intervention`/`base_mse` metrics remain computable; it is not the objective.

## Compute / GPU
- see per-run `meta.json` (device, gpu_name, batch_size_final, oom_retries, wall_sec) and `metrics.jsonl` training curves.

## Representation health
| rep | eff_rank | var_mean | cos_abs | collapsed |
|---|---|---|---|---|
| gru | 11.9 | 0.1136 | 0.356 | 0/5 |
| cpc | 10.0 | 0.0575 | 0.257 | 0/1 |
| vicreg | 5.3 | 1.0667 | 0.371 | 0/1 |
| jepa | 1.9 | 1.0313 | 0.614 | 0/5 |
| cpc@temperature-0.07 | 10.8 | 0.0426 | 0.244 | 0/1 |
| cpc@temperature-0.2 | 9.5 | 0.1196 | 0.282 | 0/1 |
| cpc@horizon-4 | 10.8 | 0.0874 | 0.280 | 0/5 |
| cpc@horizon-16 | 9.7 | 0.0400 | 0.256 | 0/1 |
| jepa@jepa_var-4.0 | 1.4 | 1.0846 | 0.824 | 1/1 |
| jepa@horizon-16 | 1.5 | 0.9281 | 0.724 | 0/1 |
| vicreg@vicreg_sim-25_vicreg_var-25 | 4.1 | 1.0856 | 0.477 | 0/1 |
| gru_untrained | 8.8 | 0.0282 | 0.389 | 0/5 |
| cpc_untrained | 8.2 | 0.0096 | 0.602 | 0/1 |
| vicreg_untrained | 7.3 | 0.0107 | 0.599 | 0/1 |
| jepa_untrained | 7.4 | 0.0096 | 0.571 | 0/5 |
| cpc@temperature-0.07_untrained | 8.2 | 0.0096 | 0.602 | 0/1 |
| cpc@temperature-0.2_untrained | 8.2 | 0.0096 | 0.602 | 0/1 |
| cpc@horizon-4_untrained | 8.9 | 0.0087 | 0.627 | 0/5 |
| cpc@horizon-16_untrained | 8.2 | 0.0096 | 0.602 | 0/1 |
| jepa@jepa_var-4.0_untrained | 7.3 | 0.0107 | 0.599 | 0/1 |
| jepa@horizon-16_untrained | 7.3 | 0.0107 | 0.599 | 0/1 |
| vicreg@vicreg_sim-25_vicreg_var-25_untrained | 7.3 | 0.0107 | 0.599 | 0/1 |

## Primary metrics (mean over seeds)
| rep | acc_in | acc_loco | acc_ood_ctx | dynseg_loco | dynseg_ood | NMI | match_ood | match_null | midctx | midctx_null | combo_oodctx | select |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| raw | 0.308 | 0.227 | 0.213 | 0.444 | 0.480 | 0.124 | 0.146 | 0.185 | 0.172 | 0.175 | 0.520 | — |
| raw_win | 0.238 | 0.203 | 0.207 | 0.466 | 0.431 | 0.086 | 0.208 | 0.164 | 0.172 | 0.168 | 0.518 | — |
| pca | 0.315 | 0.264 | 0.195 | 0.413 | 0.412 | 0.145 | 0.256 | 0.159 | 0.224 | 0.209 | 0.514 | — |
| pca_win | 0.263 | 0.224 | 0.196 | 0.379 | 0.324 | 0.097 | 0.246 | 0.149 | 0.218 | 0.210 | 0.511 | — |
| dynfeat_obs | — | — | — | — | — | — | — | — | — | — | — | — |
| dynfeat_canonical | — | — | — | — | — | — | — | — | — | — | — | — |
| gru | 0.391 | 0.282 | 0.245 | 0.339 | 0.357 | 0.103 | 0.259 | 0.172 | 0.226 | 0.205 | 0.518 | -0.574 |
| gru_untrained | 0.345 | 0.259 | 0.249 | 0.384 | 0.409 | 0.125 | 0.228 | 0.167 | 0.195 | 0.174 | 0.498 | -0.260 |
| cpc | 0.329 | 0.331 | 0.249 | 0.313 | 0.315 | 0.078 | 0.282 | 0.192 | 0.172 | 0.159 | 0.546 | -0.403 |
| cpc_untrained | 0.378 | 0.254 | 0.284 | 0.432 | 0.382 | 0.044 | 0.333 | 0.141 | 0.252 | 0.211 | 0.458 | -0.244 |
| vicreg | 0.205 | 0.229 | 0.165 | 0.266 | 0.224 | 0.014 | 0.192 | 0.090 | 0.150 | 0.141 | 0.441 | -0.612 |
| vicreg_untrained | 0.344 | 0.280 | 0.188 | 0.525 | 0.436 | 0.160 | 0.256 | 0.141 | 0.171 | 0.152 | 0.479 | -0.238 |
| jepa | 0.314 | 0.237 | 0.216 | 0.343 | 0.286 | 0.056 | 0.187 | 0.162 | 0.204 | 0.193 | 0.534 | -0.529 |
| jepa_untrained | 0.347 | 0.255 | 0.246 | 0.416 | 0.409 | 0.124 | 0.218 | 0.149 | 0.167 | 0.168 | 0.525 | -0.258 |
| cpc@temperature-0.07 | 0.359 | 0.344 | 0.246 | 0.324 | 0.291 | 0.075 | 0.256 | 0.218 | 0.189 | 0.159 | 0.541 | -0.392 |
| cpc@temperature-0.07_untrained | 0.378 | 0.254 | 0.284 | 0.432 | 0.382 | 0.044 | 0.333 | 0.141 | 0.252 | 0.211 | 0.458 | -0.244 |
| cpc@temperature-0.2 | 0.333 | 0.296 | 0.221 | 0.369 | 0.358 | 0.061 | 0.282 | 0.115 | 0.167 | 0.175 | 0.498 | -0.385 |
| cpc@temperature-0.2_untrained | 0.378 | 0.254 | 0.284 | 0.432 | 0.382 | 0.044 | 0.333 | 0.141 | 0.252 | 0.211 | 0.458 | -0.244 |
| cpc@horizon-4 | 0.386 | 0.316 | 0.283 | 0.352 | 0.335 | 0.117 | 0.285 | 0.167 | 0.251 | 0.220 | 0.523 | -0.549 |
| cpc@horizon-4_untrained | 0.417 | 0.272 | 0.246 | 0.374 | 0.379 | 0.050 | 0.313 | 0.167 | 0.238 | 0.212 | 0.508 | -0.262 |
| cpc@horizon-16 | 0.307 | 0.283 | 0.228 | 0.325 | 0.358 | 0.039 | 0.218 | 0.218 | 0.170 | 0.163 | 0.489 | -0.374 |
| cpc@horizon-16_untrained | 0.378 | 0.254 | 0.284 | 0.432 | 0.382 | 0.044 | 0.333 | 0.141 | 0.252 | 0.211 | 0.458 | -0.244 |
| jepa@jepa_var-4.0 | 0.263 | 0.221 | 0.216 | 0.310 | 0.255 | 0.051 | 0.192 | 0.115 | 0.118 | 0.133 | 0.485 | -0.415 |
| jepa@jepa_var-4.0_untrained | 0.344 | 0.280 | 0.188 | 0.525 | 0.436 | 0.160 | 0.256 | 0.141 | 0.171 | 0.152 | 0.479 | -0.238 |
| jepa@horizon-16 | 0.278 | 0.247 | 0.243 | 0.336 | 0.303 | 0.052 | 0.179 | 0.141 | 0.143 | 0.135 | 0.498 | -0.429 |
| jepa@horizon-16_untrained | 0.344 | 0.280 | 0.188 | 0.525 | 0.436 | 0.160 | 0.256 | 0.141 | 0.171 | 0.152 | 0.479 | -0.238 |
| vicreg@vicreg_sim-25_vicreg_var-25 | 0.243 | 0.244 | 0.160 | 0.227 | 0.279 | 0.017 | 0.141 | 0.115 | 0.209 | 0.163 | 0.466 | -0.548 |
| vicreg@vicreg_sim-25_vicreg_var-25_untrained | 0.344 | 0.280 | 0.188 | 0.525 | 0.436 | 0.160 | 0.256 | 0.141 | 0.171 | 0.152 | 0.479 | -0.238 |

## Trained − untrained (paired per-seed diffs)
| pair | acc_in | acc_loco | acc_ood_ctx | dynseg_acc_loco | dynseg_acc_ood_ctx | match_ood | midctx_acc | nmi_pooled | combo_oodctx_auc | intervention_selectivity |
|---|---|---|---|---|---|---|---|---|---|---|
| gru - gru_untrained | +0.046 (5/5) | +0.022 (3/5) | -0.004 (2/5) | -0.045 (1/5) | -0.052 (0/5) | +0.031 (3/5) | +0.031 (3/5) | -0.022 (1/5) | +0.021 (3/5) | -0.314 (0/5) |
| cpc - cpc_untrained | -0.050 (0/1) | +0.077 (1/1) | -0.035 (0/1) | -0.119 (0/1) | -0.067 (0/1) | -0.051 (0/1) | -0.080 (0/1) | +0.034 (1/1) | +0.088 (1/1) | -0.159 (0/1) |
| vicreg - vicreg_untrained | -0.139 (0/1) | -0.051 (0/1) | -0.023 (0/1) | -0.259 (0/1) | -0.212 (0/1) | -0.064 (0/1) | -0.021 (0/1) | -0.146 (0/1) | -0.038 (0/1) | -0.373 (0/1) |
| jepa - jepa_untrained | -0.033 (1/5) | -0.018 (2/5) | -0.030 (1/5) | -0.073 (0/5) | -0.124 (0/5) | -0.031 (1/5) | +0.037 (3/5) | -0.068 (0/5) | +0.009 (3/5) | -0.270 (0/5) |
| cpc@temperature-0.07 - cpc@temperature-0.07_untrained | -0.019 (0/1) | +0.090 (1/1) | -0.038 (0/1) | -0.107 (0/1) | -0.091 (0/1) | -0.077 (0/1) | -0.064 (0/1) | +0.031 (1/1) | +0.083 (1/1) | -0.148 (0/1) |
| cpc@temperature-0.2 - cpc@temperature-0.2_untrained | -0.046 (0/1) | +0.042 (1/1) | -0.063 (0/1) | -0.062 (0/1) | -0.024 (0/1) | -0.051 (0/1) | -0.086 (0/1) | +0.017 (1/1) | +0.040 (1/1) | -0.141 (0/1) |
| cpc@horizon-4 - cpc@horizon-4_untrained | -0.031 (1/5) | +0.044 (3/5) | +0.037 (3/5) | -0.022 (2/5) | -0.044 (1/5) | -0.028 (0/5) | +0.013 (3/5) | +0.068 (5/5) | +0.015 (3/5) | -0.288 (0/5) |
| cpc@horizon-16 - cpc@horizon-16_untrained | -0.071 (0/1) | +0.028 (1/1) | -0.055 (0/1) | -0.106 (0/1) | -0.024 (0/1) | -0.115 (0/1) | -0.082 (0/1) | -0.005 (0/1) | +0.032 (1/1) | -0.131 (0/1) |
| jepa@jepa_var-4.0 - jepa@jepa_var-4.0_untrained | -0.082 (0/1) | -0.059 (0/1) | +0.028 (1/1) | -0.215 (0/1) | -0.182 (0/1) | -0.064 (0/1) | -0.053 (0/1) | -0.110 (0/1) | +0.007 (1/1) | -0.176 (0/1) |
| jepa@horizon-16 - jepa@horizon-16_untrained | -0.066 (0/1) | -0.033 (0/1) | +0.055 (1/1) | -0.189 (0/1) | -0.133 (0/1) | -0.077 (0/1) | -0.029 (0/1) | -0.108 (0/1) | +0.020 (1/1) | -0.191 (0/1) |
| vicreg@vicreg_sim-25_vicreg_var-25 - vicreg@vicreg_sim-25_vicreg_var-25_untrained | -0.101 (0/1) | -0.036 (0/1) | -0.028 (0/1) | -0.298 (0/1) | -0.158 (0/1) | -0.115 (0/1) | +0.037 (1/1) | -0.143 (0/1) | -0.013 (0/1) | -0.310 (0/1) |

## Hypothesis verdicts
| hypothesis | criterion | methods passing | verdict |
|---|---|---|---|
| G4-H1 midctx | trained-untrained >= +0.1 and > null+0.05 | none | FAIL |
| G4-H2 match_ood | trained-untrained >= +0.1 and > null+0.05 | none | FAIL |
| G4-H3 dynseg_ood | > raw & > untrained twin & > old GRU predictor | none | FAIL |
| G4-H4 combo_oodctx | AUC >= 0.7 | none | FAIL |
| G4-H5 intervention | selectivity > 0 and > untrained twin | none | FAIL |

## Per-seed results
| rep | seed | acc_in | acc_ood_ctx | midctx | match_ood | combo_oodctx |
|---|---|---|---|---|---|---|
| gru | 0 | 0.377 | 0.248 | 0.177 | 0.282 | 0.449 |
| gru | 1 | 0.359 | 0.188 | 0.206 | 0.205 | 0.528 |
| gru | 2 | 0.460 | 0.275 | 0.303 | 0.269 | 0.528 |
| gru | 3 | 0.386 | 0.255 | 0.256 | 0.244 | 0.592 |
| gru | 4 | 0.372 | 0.262 | 0.187 | 0.295 | 0.496 |
| cpc | 0 | 0.329 | 0.249 | 0.172 | 0.282 | 0.546 |
| vicreg | 0 | 0.205 | 0.165 | 0.150 | 0.192 | 0.441 |
| jepa | 0 | 0.259 | 0.252 | 0.170 | 0.179 | 0.494 |
| jepa | 1 | 0.337 | 0.221 | 0.214 | 0.218 | 0.585 |
| jepa | 2 | 0.354 | 0.209 | 0.275 | 0.179 | 0.569 |
| jepa | 3 | 0.336 | 0.187 | 0.231 | 0.167 | 0.507 |
| jepa | 4 | 0.284 | 0.211 | 0.130 | 0.192 | 0.516 |
| cpc@temperature-0.07 | 0 | 0.359 | 0.246 | 0.189 | 0.256 | 0.541 |
| cpc@temperature-0.2 | 0 | 0.333 | 0.221 | 0.167 | 0.282 | 0.498 |
| cpc@horizon-4 | 0 | 0.363 | 0.243 | 0.284 | 0.295 | 0.480 |
| cpc@horizon-4 | 1 | 0.343 | 0.246 | 0.201 | 0.295 | 0.543 |
| cpc@horizon-4 | 2 | 0.420 | 0.381 | 0.259 | 0.282 | 0.596 |
| cpc@horizon-4 | 3 | 0.458 | 0.340 | 0.355 | 0.295 | 0.546 |
| cpc@horizon-4 | 4 | 0.349 | 0.203 | 0.159 | 0.256 | 0.450 |
| cpc@horizon-16 | 0 | 0.307 | 0.228 | 0.170 | 0.218 | 0.489 |
| jepa@jepa_var-4.0 | 0 | 0.263 | 0.216 | 0.118 | 0.192 | 0.485 |
| jepa@horizon-16 | 0 | 0.278 | 0.243 | 0.143 | 0.179 | 0.498 |
| vicreg@vicreg_sim-25_vicreg_var-25 | 0 | 0.243 | 0.160 | 0.209 | 0.141 | 0.466 |
| gru_untrained | 0 | 0.332 | 0.247 | 0.156 | 0.231 | 0.484 |
| gru_untrained | 1 | 0.302 | 0.218 | 0.229 | 0.231 | 0.537 |
| gru_untrained | 2 | 0.366 | 0.294 | 0.177 | 0.269 | 0.510 |
| gru_untrained | 3 | 0.362 | 0.205 | 0.311 | 0.192 | 0.491 |
| gru_untrained | 4 | 0.364 | 0.284 | 0.102 | 0.218 | 0.466 |
| cpc_untrained | 0 | 0.378 | 0.284 | 0.252 | 0.333 | 0.458 |
| vicreg_untrained | 0 | 0.344 | 0.188 | 0.171 | 0.256 | 0.479 |
| jepa_untrained | 0 | 0.344 | 0.188 | 0.171 | 0.256 | 0.479 |
| jepa_untrained | 1 | 0.324 | 0.285 | 0.223 | 0.179 | 0.577 |
| jepa_untrained | 2 | 0.377 | 0.247 | 0.143 | 0.192 | 0.503 |
| jepa_untrained | 3 | 0.353 | 0.264 | 0.192 | 0.179 | 0.542 |
| jepa_untrained | 4 | 0.334 | 0.246 | 0.106 | 0.282 | 0.526 |
| cpc@temperature-0.07_untrained | 0 | 0.378 | 0.284 | 0.252 | 0.333 | 0.458 |
| cpc@temperature-0.2_untrained | 0 | 0.378 | 0.284 | 0.252 | 0.333 | 0.458 |
| cpc@horizon-4_untrained | 0 | 0.378 | 0.284 | 0.252 | 0.333 | 0.458 |
| cpc@horizon-4_untrained | 1 | 0.382 | 0.198 | 0.188 | 0.295 | 0.529 |
| cpc@horizon-4_untrained | 2 | 0.519 | 0.294 | 0.268 | 0.308 | 0.479 |
| cpc@horizon-4_untrained | 3 | 0.386 | 0.236 | 0.212 | 0.308 | 0.600 |
| cpc@horizon-4_untrained | 4 | 0.421 | 0.217 | 0.272 | 0.321 | 0.475 |
| cpc@horizon-16_untrained | 0 | 0.378 | 0.284 | 0.252 | 0.333 | 0.458 |
| jepa@jepa_var-4.0_untrained | 0 | 0.344 | 0.188 | 0.171 | 0.256 | 0.479 |
| jepa@horizon-16_untrained | 0 | 0.344 | 0.188 | 0.171 | 0.256 | 0.479 |
| vicreg@vicreg_sim-25_vicreg_var-25_untrained | 0 | 0.344 | 0.188 | 0.171 | 0.256 | 0.479 |

## Comparison with G0-v3
| rep | acc_ood_ctx | dynseg_ood | match_ood | midctx | combo_oodctx |
|---|---|---|---|---|---|
| g0:gru | 0.237 | 0.346 | 0.238 | 0.206 | 0.514 |
| g0:gru_untrained | 0.280 | 0.419 | 0.246 | 0.204 | 0.555 |
| g0:ae | 0.285 | 0.368 | 0.285 | 0.186 | 0.532 |
| g0:ae_untrained | 0.246 | 0.331 | 0.315 | 0.253 | 0.521 |
| v4:gru | 0.245 | 0.357 | 0.259 | 0.226 | 0.518 |
| v4:cpc | 0.249 | 0.315 | 0.282 | 0.172 | 0.546 |
| v4:vicreg | 0.165 | 0.224 | 0.192 | 0.150 | 0.441 |
| v4:jepa | 0.216 | 0.286 | 0.187 | 0.204 | 0.534 |
| v4:cpc@temperature-0.07 | 0.246 | 0.291 | 0.256 | 0.189 | 0.541 |
| v4:cpc@temperature-0.2 | 0.221 | 0.358 | 0.282 | 0.167 | 0.498 |
| v4:cpc@horizon-4 | 0.283 | 0.335 | 0.285 | 0.251 | 0.523 |
| v4:cpc@horizon-16 | 0.228 | 0.358 | 0.218 | 0.170 | 0.489 |
| v4:jepa@jepa_var-4.0 | 0.216 | 0.255 | 0.192 | 0.118 | 0.485 |
| v4:jepa@horizon-16 | 0.243 | 0.303 | 0.179 | 0.143 | 0.498 |
| v4:vicreg@vicreg_sim-25_vicreg_var-25 | 0.160 | 0.279 | 0.141 | 0.209 | 0.466 |

## Negative results & limitations
- see per-run eval.json for full metric sets; collapsed runs (health.collapsed) are flagged in Representation health.
- claim ceiling (spec §26): even on PASS the defensible claim is that a label-free predictive/self-supervised objective produced sensor-context-invariant, reusable latent-cause representations beyond untrained reservoirs, raw features and shuffle nulls — not that symbolic concepts emerged.
- on FAIL: reconstruction-vs-prediction is largely excluded as the explanation; the likely missing ingredients are embodied consequence / active intervention / need-relevance / action-contingent structure (spec §27).
