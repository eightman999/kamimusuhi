# T0 — Temporal Sense

外部時計を持たないエージェント内部に、未知の時間間隔にも利用可能な「経過時間」の表現は形成されるか、を検証する実験です。K0-E2 で観測された「delay=47 では機能するが未知時刻へ転移しない」問題を独立して切り出します。

実測結果と仮説判定は [reports/T0_RESULTS.md](reports/T0_RESULTS.md) に集約します。

**Protocol: T0-v2**。v1 (PR #28 初版) には (1) interpolation 評価 delay が training 分布に混入、(2) T-C3 intervention が target step に同期、(3) eval が要求 delay を環境に適用していない、という protocol defect があり、全 v1 数値を破棄しました ([reports/T0_V1_INVALIDATION.md](reports/T0_V1_INVALIDATION.md))。v2 では `38–42` / `54–58` の holdout band を `excluded_training_delays` で training support から除去し、protocol validator が評価・集約・report 生成を gate します。

**スコープ**: 主実験 T0-A は v2 で学習・評価・介入・probe まで実測済みです。T0-B/C/D は環境・oracle・smoke test の実装がある段階で、学習済み実験結果はありません。判定表の PASS/FAIL は T0-A にのみ適用されます。

## 時間情報の非リーク条件

agent の観測には以下を一切含みません。

- current timestep / absolute time
- remaining time / normalized progress
- 環境時刻から導出した position encoding

時間は内部状態から推定しなければなりません。観測に含まれるのは、episode ごとに位相がランダムな発振子と平均回帰ランダムウォークという **確率的な世界ダイナミクス** であり、固定 step index からは一意に時刻を復元できません( [tests/test_leak.py](tests/test_leak.py) で検証)。これらのチャネルは「状態変化に基づく時間推定」の経路として残し、T-C5 で世界速度を変えて、内部時計が step counter なのか状態変化ベースなのかを判別します。

## 主課題: T0-A Interval Production

```text
t=0: 刺激パルス (cue channel に delay 強度 D/128 を2step提示)
↓    delay D step (観測はダイナミクスとノイズのみ)
t=T*: 応答窓 |t - T*| <= window(=2)
```

`T* = round(D / time_scale)` で、訓練時 `time_scale=1` です。行動は `WAIT / EXPECT / ACT` の3つで、最初の `ACT` が commit になります。報酬は `clip(1 - |t_act - T*| / 8, -1, 1)` で graded、窓内は +0.75 以上、act せずに horizon 到達は -1 です。全 step に `step_cost=0.01` があるため、何もしないは常に劣ります。

教師 (oracle) は `t >= T*` で `ACT` を1回だけ出し、模倣学習のラベルに使います。

## 補助課題 (env/temporal_tasks.py)

| task | 内容 | 回答 |
|---|---|---|
| `comparison` (T0-B) | 4つのmarker pulse が2区間を作る。どちらが長いか | `LONGER_FIRST / LONGER_SECOND` |
| `order` (T0-C) | 異なる channel の pulse A/B がランダム時刻に出る。どちらが先か | `A_FIRST / B_FIRST` |
| `delayed_response` (T0-D) | t=0 の二値 cue を覚え、delay 後の窓内で回答 | `ANSWER_A / ANSWER_B` |

## Delay 条件

訓練 grid: `8, 16, 24, 32, 48, 64`。`delay_distribution` は `grid / uniform / geometric / mixed` を取り、訓練は `mixed` (grid 50% + uniform[8,64] 50%) で固定周期の丸暗記を防ぎます。

v2 では `excluded_training_delays` が全分布の sampler を constrain し、interpolation holdout band (`38–42`, `54–58`) は training support に含まれません。grid に excluded delay があれば config error、uniform/geometric/mixed は allowed set からのみ sample します。`reset(delay_override=...)` は除外の影響を受けないため、評価は holdout delay を強制できます。

評価 split (v2 spec §12):

- seen: 訓練 grid `8,16,24,32,48,64`
- interpolation primary: `40, 56` (held-out)
- interpolation band: `38–42, 54–58` 全10点 (held-out robustness)
- extrapolation: `80, 96, 128` (最大2倍)
- distractor: `distractor_rate=0.2`
- scaled: `time_scale` を `0.5 / 2.0` に変更 (T-C5)。世界ダイナミクスの速度が変わり、正解 ACT step も `round(D/s)` に移動します。step counter は失敗し、状態変化ベースの時計だけが追随します。

## モデル

`models/` の全アーキテクチャは共通 interface です: `initial_state(B)` は単一 tensor `[B,H]` (MLP は `[B,0]`)、`forward(obs, state) -> (logits, value, state)`、`forward_sequence(obs[T,B,C], state)`。state を1本の tensor に揃えることで、hidden reset / noise / quantization 介入を全構造で同一に適用できます (LSTM は `h||c` を連結して保持)。

| arch | 実装 |
|---|---|
| `mlp` | 2層 tanh encoder、memoryless baseline (T0-H1) |
| `gru64` / `gru128` | nn.GRU |
| `lstm64` / `lstm128` | nn.LSTM、h‖c packed state |
| `ssm` | 実対角 SSM (log-spaced decay を学習、gate なし) — より線形な時間表現の仮説 |
| `leaky` | 固定 rate の leaky integrator bank + 学習 readout、baseline |

## 学習

```bash
python -m experiments.t0.train --config experiments/t0/configs/train_default.json \
    --artifacts experiments/t0/artifacts/primary_v2 --run-id gru64-s0
```

k0 系の慣例に従い、oracle への全系列模倣 (120 update、後半は 80% teacher mix) のあと系列分割なし PPO (80 update) を行います。checkpoint は `initial / imitation_best / imitation_final / ppo_best / ppo_final`、選択基準は固定 validation seed `700001` の `success + 0.1 * reward` です。RNG stream は imitation `100000+seed` / PPO `200000+seed` / eval `900001` で分離しています。

## sweep

```bash
python -m experiments.t0.sweep \
  --artifacts experiments/t0/artifacts/primary_v2 \
  --seeds 0,1,2 \
  --architectures mlp,gru64,gru128,lstm64,lstm128,ssm,leaky \
  --evaluate --device cpu
```

完了 run は `status.json` を見て skip します。`--smoke` で短縮一巡。

## 評価

```bash
python -m experiments.t0.evaluate --checkpoint <run>/ppo_best.pt \
    --artifacts experiments/t0/artifacts/primary_v2 --episodes 128
python -m experiments.t0.evaluate --consolidate-only \
    --artifacts experiments/t0/artifacts/primary_v2
python -m experiments.t0.analysis.report \
    --artifacts experiments/t0/artifacts/primary_v2 \
    --out experiments/t0/reports/T0_RESULTS.md
```

primary metrics: `timing error`, `mean |error|`, `success rate`, `early/late action rate`。causal tests (v2): T-C1 hidden reset、T-C2 hidden noise、T-C3a `post_cue_blank` (cue 後に定数化、解除なし・target step 非参照)、T-C3b `freeze_dynamics` (episode 固有の post-cue obs を凍結して再生)、T-C4 distractor 挿入、T-C5 temporal scaling。representation analysis は `analysis/probes.py` の linear probe (elapsed / remaining / phase の held-out episode R² + shuffle 対照) と `analysis/trajectories.py` の PCA です。可視化は判定に使いません。

`analysis/protocol.py` の validator が評価・集約・report 生成の前に
train/eval split と intervention の target-independence を検査し、違反時は
`INVALID_PROTOCOL` で停止します。判定ゲートは `analysis/report.py` 冒頭の
pre-registered threshold に固定されています。

## 判定

spec §13/§14 に従い、PASS 条件 (recurrent > MLP、unseen interpolation 成功、hidden reset で性能低下、hidden から elapsed time probe 可能、複数 seed で再現) と Strong PASS (extrapolation、2倍転移、distractor 耐性、temporal scaling 適応、SSM の線形時間表現) を測定値からのみ判定します。これらの判定は主実験 T0-A に対するもので、aux task には適用しません。

## テスト

```bash
python -m unittest discover -s experiments/t0/tests
```

`test_env` (動的・報酬・決定性・snapshot)、`test_leak` (§1 の禁止事項の検査)、`test_models` (shape・系列/逐次の一致)、`test_train_eval` (短縮 train・介入・consolidate) があります。
