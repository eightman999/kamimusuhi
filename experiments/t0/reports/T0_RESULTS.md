# T0 Results — Temporal Sense

> 外部時計を持たないエージェント内部に、未知の時間間隔にも利用可能な「経過時間」の表現は形成されるか。

本ファイルは `experiments/t0/artifacts/primary/` の実測値から転記した判定記録です。集計元: `eval/*_ppo_best.json`, `consolidated.json` (3 seed 平均・SD・median・bootstrap 95% CI)。実装と条件の正典は [../README.md](../README.md)。

## 条件

- 主課題: T0-A Interval Production (t=0 の cue 強度 D/128 を保持し、`T*=round(D/time_scale)` で `ACT`、窓 ±2)
- 訓練: oracle 模倣 120 update + PPO 80 update、512 env、`mixed` 分布 (grid `8,16,24,32,48,64` 50% + uniform[8,64] 50%)、horizon 96
- 評価: delay ごとに greedy 128 episode、eval seed `900001`、checkpoint は固定 validation `700001` で選択した `ppo_best.pt`
- arch: mlp / gru64 / gru128 / lstm64 / lstm128 / ssm / leaky × seed 0,1,2

## 結果 (3 seed 平均 success rate)

| arch | seen | interpolation (40,56) | extrapolation (80,96,128) | hidden reset | elapsed R² |
|---|---:|---:|---:|---:|---:|
| mlp | .075 | .051 | .079 | .060 | n/a (state なし) |
| gru64 | .986 | .991 | .993 | .073 | .923 |
| gru128 | .997 | .997 | .998 | .049 | .980 |
| lstm64 | .553 | .566 | .519 | .000 | .932 |
| lstm128 | .702 | .738 | .690 | .039 | .971 |
| ssm | .093 | .116 | .106 | .120 | ≈1.000 |
| leaky | .090 | .090 | .088 | .120 | ≈1.000 |

介入・ストレス (delay=32、3 seed 代表値):

| arch | distractor .2 | scale ×0.5 | scale ×2.0 | obs blank | hidden noise |
|---|---:|---:|---:|---:|---:|
| gru64 | .95–1.00 | .00 | .00 | .00 | .21–.30 |
| gru128 | .93–1.00 | .00 | .00 | .00 | .30–.31 |
| lstm64 | .47–.59 | .00 | .00 | .00 | .40–.50 |
| lstm128 | .30–.89 | .00 | .00 | .00 | .30–.85 |
| ssm / leaky / mlp | ≲.11 (床) | ≲.07 | ≲.19 | — | — |

## 仮説判定

| 仮説 | 内容 | 判定 | 根拠 |
|---|---|---|---|
| T0-H1 | recurrent > MLP | **PASS** | gru 0.99 / lstm 0.55–0.70 vs mlp 0.075 (paired 3/3 wins、n=3 では sign test p≤0.25 が下限) |
| T0-H2 | unseen interpolation | **PASS** | gru ≈0.99、lstm 0.57–0.74 (delay 40/56 は訓練 grid に非含有) |
| T0-H3 | 一部 model で extrapolation | **PASS (Strong)** | gru は 128 step (訓練最大 64 の2倍) でも ≈0.99。cue 強度 1.0 自体も訓練範囲外 |
| T0-H4 | hidden reset で崩壊 | **PASS** | gru 1.00→0.05–0.08、lstm 0.55–0.70→0.00–0.12。reset 後は即 ACT (mean error ≈ −T*/2) で、経過時間が hidden に宿ることを示唆 |
| T0-H5 | hidden から elapsed probe | **PASS** | held-out episode R² = 0.92–1.00 (shuffle 対照 <0)。ssm/leaky は R²≈1.000 |

総合: **PASS** (全5条件)。Strong PASS は extrapolation・2倍転移・distractor 耐性・SSM 線形時間表現を達成、**temporal scaling 適応は全 model で FAIL**。

## 主な観察

- **学習された時計は step counter**: T-C5 で `time_scale` を 0.5/2.0 にすると全モデルが完全に崩壊 (success 0.00)。世界ダイナミクスの速度変化に追従せず、|error| はほぼ `D·|1−1/s|` 相当 → 経過時間を環境ステップ数で数えている。
- **表現と利用の解離 (ssm / leaky)**: 線形 probe で elapsed time R²≈1.0 と最もきれいな時間表現を持つが、課題成功率は床 (~0.1)。時間は線形に符号化されるが、readout がそれを条件付き ACT へ変換できていない。gate 機構の役割を示唆。
- **LSTM < GRU**: 同一条件で LSTM は GRU に明確に劣り seed 分散も大きい (0.34–0.94)。容量ではなく学習安定性の差。
- **obs blank (T-C3)**: GRU/LSTM は delay 中の観測を定数化すると全滅 (0.00、平均 +40 step の遅行動)。dynamics channel が時計の進行を部分的に駆動しているが、速度変化には適応しない — 状態変化を「補助入力」として使いつつ「絶対速度」には学習済みレートを仮定している解釈と整合。
- **mlp の obs blank での見かけ上の改善** (0.06→0.30–0.41): blank が t=T* で終わるため、窓直後に観測分布が変化し、その入力差が ACT を誘発する artifact。memoryless model が timing 情報を入力遷移から拾った例で、内部時計ではない。
- **訓練ダイナミクス**: 模倣のみでは success ≈0.14/|e|≈15 に頭打ち、PPO の graded reward が精度を作る (途中で never-ACT へ一旦 collapse → 回復)。`ppo_best` の validation 選択が実効上重要。

## FAIL 条件との照合 (spec §14)

- seen delay だけ暗記 → **該当せず** (interp/extrap が seen と同等)
- hidden reset で不変 → **該当せず**
- 刺激直後から固定 action pattern → **該当せず** (行動タイミングが delay 依存)
- unknown delay で collapse → **GRU では該当せず**、ssm/leaky は訓練内でも collapse
- absolute clock 相当のリーク → **検査済み** (`tests/test_leak.py`: 決定的な time-varying channel なし、線形 ramp R²<0.5、dynamics channel は episode 間分散あり)

## 未実施 / 残課題

- aux task (T0-B/C/D) は環境と oracle smoke のみ実施。学習・評価は未実施
- PPO 中盤の never-ACT collapse の原因切り分け (value warm-up、entropy schedule) は未分析
- distractor 耐性は rate 0.2 の1条件のみ
- 補助: `interval` 以外の task では T-C5 semantics が課題固有になるため要再設計
