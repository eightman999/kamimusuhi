# T0 Results — Temporal Sense

> 外部時計を持たないエージェント内部に、未知の時間間隔にも利用可能な「経過時間」の表現は形成されるか。

**Protocol version: T0-v2** — held-out interpolation bands を training support から除去した再測定版。v1 protocol の欠陥と撤回済み解釈は [T0_V1_INVALIDATION.md](T0_V1_INVALIDATION.md) を参照。本ファイルは `experiments/t0/analysis/report.py` により v2 artifact のみから生成される。

**適用範囲**: 以下の全数値・判定は主実験 **T0-A (interval production)** のものです。T0-B/C/D は env + oracle + smoke test のみで、学習済み実験結果ではありません。

## 条件

- source commit: `70f5ac886c11bc7d2cf6b193d2e1a2651c1b0c2a`
- architectures: gru128, gru64, leaky, lstm128, lstm64, mlp, ssm / seeds: 0, 1, 2
- training support (sampler が生成しうる delay): [8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 53, 59, 60, 61, 62, 63, 64]
- excluded_training_delays (interpolation holdout): [38, 39, 40, 41, 42, 54, 55, 56, 57, 58]
- interpolation primary: [40, 56]; holdout bands: [38, 39, 40, 41, 42, 54, 55, 56, 57, 58]; extrapolation: [80, 96, 128]
- validation seed (model selection): 700001; eval seed: 900001; 選択基準: `success + 0.1 * reward` on fixed validation grid
- device/backend: `cpu` (M2 Max; MPS は小模型・batch env で ~7x 低速のため未採用)
- protocol integrity: **PASS** (n_eval_rows=21, train/interp overlap=[], train/holdout overlap=[], train/extrap overlap=[], val/test overlap=[], intervention target-independence={'post_cue_blank': True, 'freeze_dynamics': True})

## 結果 (seed 平均 success rate)

| arch | seen | interp (40,56) | interp band | extrap | hidden reset | post-cue blank | freeze dyn | elapsed R² |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| gru128 | .530 | .432 | .428 | .176 | .229 | .000 | .698 | .998 |
| gru64 | .946 | .953 | .962 | .260 | .000 | .000 | .995 | .993 |
| leaky | .108 | .010 | .008 | .000 | .190 | .000 | .042 | 1.000 |
| lstm128 | .158 | .005 | .003 | .000 | .000 | .000 | .102 | -421.273 |
| lstm64 | .761 | .435 | .445 | .016 | .000 | .000 | .852 | .958 |
| mlp | .078 | .030 | .030 | .003 | .094 | .000 | .000 | n/a |
| ssm | .139 | .000 | .001 | .000 | .227 | .000 | .060 | .999 |

ストレス (seed 平均):

| arch | distractor .2 | scale ×0.5 | scale ×2.0 | hidden noise |
|---|---:|---:|---:|---:|
| gru128 | .525 | .056 | .085 | .164 |
| gru64 | .914 | .000 | .000 | .242 |
| leaky | .112 | .059 | .153 | .049 |
| lstm128 | .153 | .075 | .118 | .107 |
| lstm64 | .706 | .000 | .000 | .760 |
| mlp | .078 | .040 | .167 | .094 |
| ssm | .139 | .069 | .155 | .076 |

## 仮説判定 (pre-registered gates)

| 仮説 | gate | 判定 | 実測 |
|---|---|---|---|
| T0-G0 | protocol integrity clean | PASS | PASS |
| T0-H1 | gru/lstm 全arch seen > mlp | PASS | mlp seen=.078 |
| T0-H2 | held-out interp ≥0.8 かつ band ≥0.7 | PASS | interp=.953, band=.962 |
| T0-H3 | extrap ≥0.8 (strong) | FAIL | best extrap=.260 |
| T0-H4 | baseline−reset ≥0.5 | PASS | gru64: .997→.000 |
| T0-H5 | elapsed R²≥0.8, shuffle<0.3 | PASS | best R²=1.000, shuffle=-0.729 |

Strong PASS 構成要素: extrapolation=FAIL, distractor=PASS, temporal_scaling=FAIL, ssm_linear_time=PASS

T-C3 case (best viable arch `gru64`): **Case A-** — freeze では timing が完全に保たれ、constant-fill blank のみ崩壊 — 内部 recurrence は timing を駆動できるが、0.5 充填は学習分布外入力として機能を破壊する。dynamics 依存の証拠ではない

## 総合判定: **PASS**

## 観察

- 使用 checkpoint: imitation_best.pt, ppo_best.pt (imitation stage が validation best だった run は imitation_best を使用)
- `gru64` seen_mean_success: seed range .908–.993
- `gru64` interpolation_mean_success: seed range .879–.996
- `gru64` extrapolation_mean_success: seed range .005–.529
- `post_cue_blank` は全 arch で 0.00 — 0.5 充填は学習分布外入力であり、`freeze_dynamics` (episode 固有 obs 凍結) との対比が dynamics 依存性の判別子となる (gru64: freeze=.995 / blank=.000)
- temporal scaling は依然として全 arch でほぼ 0 — 学習された時計はworld-speed に適応しない step counter

## Limitations

- aux task (T0-B/C/D) は env + oracle + smoke test のみ。学習済み結果はない
- T-C5 (temporal scaling) の結果は step counter vs world-clock の判別子であり、適応の成否は strong verdict のみに影響する
- probe は線形 readout の存在証明であり因果利用の証明ではない
- v1 の数値は protocol defect により全て破棄。比較・継続使用はしない ([T0_V1_INVALIDATION.md](T0_V1_INVALIDATION.md))

