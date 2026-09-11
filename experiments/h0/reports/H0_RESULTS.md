# H0 — Artificial Homeostasis: Results

**Verdict: PASS** (5/5 PASS criteria)

## PASS criteria

| criterion | result | detail |
|---|---|---|
| PASS-1 learned<random | PASS | gru64 err=0.0333 vs random 0.2418, perm p=0.0002 |
| PASS-2 >=heuristic or OOD win | PASS | ID 0.0333 vs heur 0.0275; OOD wins: ['ood_energy', 'ood_temperature', 'ood_risk'] |
| PASS-3 shuffle degrades | PASS | mean Δerr=+0.1192 over 5 seeds |
| PASS-4 >=2 state-dependent vars | PASS | action_shift={'energy': 0.191, 'fatigue': 0.336, 'novelty': 0.091, 'certainty': 0.057, 'temperature': 0.067} MI={'energy': 0.037, 'fatigue': 0.001, 'novelty': 0.011, 'certainty': 0.007, 'temperature': 0.008} |
| PASS-5 OOD recovery > fixed | PASS | OOD wins: ['ood_energy', 'ood_temperature', 'ood_risk'] |

## In-distribution (base env)

| agent | ckpt | err_full | survival | stable | entropy |
|---|---|---|---|---|---|
| random | - | 0.2418 | 0.472 | 0.190 | 0.995 |
| heuristic | - | 0.0275 | 1.000 | 0.659 | 0.650 |
| mlp64 | pre | 0.4448±0.097 | 0.436 | 0.120 | 0.248 |
| mlp64 | best | 0.0417±0.011 | 1.000 | 0.177 | 0.622 |
| mlp64 | final | 0.0728±0.028 | 1.000 | 0.072 | 0.593 |
| gru64 | pre | 0.6912±0.105 | 0.154 | 0.140 | 0.184 |
| gru64 | best | 0.0333±0.023 | 1.000 | 0.402 | 0.643 |
| gru64 | final | 0.0512±0.035 | 1.000 | 0.311 | 0.624 |
| gru128 | pre | 0.6139±0.141 | 0.121 | 0.171 | 0.260 |
| gru128 | best | 0.0334±0.007 | 1.000 | 0.353 | 0.639 |
| gru128 | final | 0.0620±0.048 | 0.993 | 0.283 | 0.623 |

## OOD (zero-shot, best checkpoint)

| agent | ood_energy | ood_temperature | ood_risk | ood_resource |
|---|---|---|---|---|
| random | 0.3506 | 0.2471 | 0.3295 | 0.3283 |
| heuristic | 0.1203 | 0.0414 | 0.1216 | 0.2091 |
| mlp64 | 0.0523 | 0.0447 | 0.0424 | 0.3394 |
| gru64 | 0.0433 | 0.0358 | 0.0344 | 0.3359 |
| gru128 | 0.0499 | 0.0413 | 0.0356 | 0.4040 |

## Causal ablations (Δ error_full vs baseline, best ckpt)

| ablation | mean Δerr |
|---|---|
| shuffle | 0.1388 |
| mask_energy | 0.2492 |
| mask_fatigue | 0.1581 |
| mask_novelty | 0.2616 |
| mask_certainty | 0.0183 |
| mask_temperature | -0.0067 |
| permute | 0.4914 |
| action_perm_rest_explore | 0.8031 |
| hidden_reset | -0.0001 |

## Findings / limitations

- In-distribution, the best learned agent (gru64) is above the hand-coded heuristic (0.0333 vs 0.0275). The learned advantage is out-of-distribution, not ID.
- OOD conditions where learned loses to heuristic: ood_resource (0.336 vs 0.209).
- Channels with causal influence on behavior: energy (+0.227), fatigue (+0.192), novelty (+0.217).
- Channels the policy does not causally use (mask delta ~0 and low action-shift): certainty, temperature.
- hidden_reset delta = +0.0001: no evidence of long-horizon dependence on recurrent hidden state within a 1024-step episode.
- best→final ID error regression: +0.0179 mean over 5 seeds (PPO continues to drift after the best checkpoint; best-checkpoint selection matters).

## Figures

- `figs/ablation_delta.png`
- `figs/error_vs_time.png`
- `figs/ood_comparison.png`
- `figs/trace_internal.png`
- `figs/train_gru128_seed0.png`
- `figs/train_gru64_seed0.png`
- `figs/train_mlp64_seed0.png`
- `figs/transition_matrix.png`

## Reproduce

```bash
.venv/bin/python -m experiments.h0.sweep all --seeds 0,1,2,3,4
.venv/bin/python -m experiments.h0.report
```
## Rhythm analysis (representative episode)

```json
{
  "transition_matrix": [
    [
      0.0,
      0.0,
      0.0,
      0.0,
      0.0
    ],
    [
      0.0,
      0.6480836236933798,
      0.18292682926829268,
      0.15505226480836237,
      0.013937282229965157
    ],
    [
      0.0,
      0.5758754863813229,
      0.23735408560311283,
      0.15953307392996108,
      0.027237354085603113
    ],
    [
      0.0,
      0.28125,
      0.4875,
      0.1875,
      0.04375
    ],
    [
      0.0,
      0.28125,
      0.375,
      0.03125,
      0.3125
    ]
  ],
  "dwell_times": {
    "IGNORE": 0.0,
    "WAIT": 2.8415841584158414,
    "REST": 1.3112244897959184,
    "EXPLORE": 1.2290076335877862,
    "OBSERVE": 1.4545454545454546
  },
  "action_counts": {
    "IGNORE": 0,
    "WAIT": 574,
    "REST": 257,
    "EXPLORE": 161,
    "OBSERVE": 32
  },
  "internal_ranges": {
    "energy": [
      0.40207900782276984,
      0.7564176650215528
    ],
    "fatigue": [
      0.0,
      0.16796450496031112
    ],
    "novelty": [
      0.23989121123645019,
      0.5714752206387806
    ],
    "certainty": [
      0.3498556790040073,
      0.6897649682370516
    ],
    "temperature": [
      0.3150406453938239,
      0.7126401506384359
    ]
  },
  "dominant_period": {
    "period": 100.0,
    "strength": 3.3817991437929664
  }
}
```
