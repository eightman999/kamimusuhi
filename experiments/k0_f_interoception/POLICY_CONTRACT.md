# K0-F 学習・予測・評価契約

## 入力 JSONL

schema_version は `k0-f-policy-v1`。1 行を実測タスクの資源選択 1 回とする。

```json
{
  "schema_version": "k0-f-policy-v1",
  "episode_id": "unique-decision-id",
  "block_id": "independent-workload-block",
  "split": "train",
  "task_features": [0.16666667, 0.5, 0.0002, 0.0],
  "body_sequence": [[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]],
  "body_mask_sequence": [[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]],
  "stale_body_sequence": [[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0]],
  "stale_body_mask_sequence": [[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]],
  "costs_seconds": [0.006, 0.0008, 0.0012],
  "failures": [false, false, false],
  "deadline_seconds": 0.001,
  "probe_targets": {
    "future_rtx3060_util": 0.1,
    "future_p100_util": 0.1,
    "next_job_latency_seconds": 0.0008
  },
  "provenance": {
    "telemetry_kind": "real",
    "frame_ids": [],
    "stale_age_seconds": 30,
    "target_kind": "next_job_minimum_measured_resource_completion"
  }
}
```

履歴は 1〜8 frame、古い順、body は 20 次元、値・mask とも `[0,1]`。
task features は `[matrix_dimension/3072, repetitions/20, deadline_seconds/5, precision_flag]`。
現行資源は CPU / RTX3060 / P100 の 3 action。無意味な WAIT 等を追加しない。
履歴長を GRU packing で扱い、padding を有効観測にしない。

入力 whitelist は task 4 + masked body 20 + mask 20 = 44 float。
absolute timestamp、frame/block/episode ID、workload label、costs、teacher、probe target はモデルへ渡さない。
raw telemetry・整列 frame・本 dataset は別 artifact に保存する。
split は workload block ごとに完全分離し、split 間に 35 秒の取得待機を挟む。
実測 30 秒前の STALE が取得できない場合は mask 0。正常値として補完しない。
STALE は age-aware として本収集前に固定する。古い frame を copy し、value[18] に現在までの追加 age/60 を加算して最大1へ制限する。原始 frame は改変しない。

## 予測 probe

現在最後の入力 + body 履歴平均を train-only 標準化し、固定 ridge λ=10 で予測する。
BODY / BLIND / SHUFFLED / STALE をそれぞれ train に fit する。
target は未来 10 秒後の RTX3060/P100 utilization と、次 job の実測最小 completion cost。
GPU の未来観測と、直後に測った job cost を artifact の target_kind で区別する。

validation gate は事前固定する。

- train で非定数、validation で観測可能な target が最低 2 種。
- train-target SD で正規化した MAE を target 等重みで平均。
- BODY が BLIND より 10% 以上低いときのみ policy 学習へ進む。
- この記述的 screening を因果的研究成功や telemetry point の有意差と呼ばない。
- 最初は validation のみ出力。held-out は gate 固定後に `--include-test` で追加し、旧 gate を保存する。

```sh
python -m experiments.k0_f_interoception.probe --dataset DATASET --output OUTPUT
python -m experiments.k0_f_interoception.policy --dataset DATASET --output OUTPUT --probe OUTPUT/prediction_probe.json --normalization-config NORMALIZATION --stage train
python -m experiments.k0_f_interoception.probe --dataset DATASET --output OUTPUT --include-test
python -m experiments.k0_f_interoception.policy --dataset DATASET --output OUTPUT --probe OUTPUT/prediction_probe.json --normalization-config NORMALIZATION --stage evaluate
```

## 学習

GRU128、seed 0〜7。各 seed で BODY と独立 BLIND を同じ初期パラメータ、タスク順で学習する。
デフォルト 160 epochs、batch 64、Adam lr=0.001、gradient clip 1.0。
imitation loss = teacher cross entropy + softmax action の期待 utility regret。
teacher は実測 utility の argmax。

`utility = 1 - min(measured_latency_seconds / deadline_seconds, 2)`、実測 failure は `-1`。
loss/deadline/teacher の定義を held-out 結果に合わせて変更しない。
validation の平均 utility 最大を best に選び、同点は最初の checkpoint を保持する。
best と final を分離して保存し、上書きを拒否する。
同じ凍結 training_config で再実行した場合だけ、status・source/data/normalization・checkpoint hash の一致する完了 run を再利用する。中断した未完了 run は上書きせず停止する。
checkpoint に architecture、seed、parent=null、stage、source commit、source/data/normalization identity を保存する。

1 episode は 1 回の資源選択で、replay の action は次 row の状態を変えないため DAgger は実施しない。
PPO は実施しない。これは closed-loop 環境 RL の代替的証拠ではない。

## Primary と secondary

Primary は同じ BODY-best checkpoint の BODY / BLIND / SHUFFLED / STALE 入力介入。
SHUFFLED は同 split 内の別 workload block との body 履歴の全単射で、body 周辺分布を保存する。
task と evaluation row 順は保持する。seed ごとに決定的 shuffle を用いる。
独立 BLIND の最良 checkpoint 比較は secondary。情報除去による分布外入力と、最初から情報のない学習を区別する。

各 seed の同じ held-out row 平均を独立単位とする。
mean、sample SD、median、20,000 回 seed bootstrap 95% CI、paired exact sign test を保存する。
primary 各差の条件は n>=8、mean>0、CI lower>0、two-sided exact sign p<=0.05。
すべての primary 条件の conjunction を判定し、失敗した条件を隠さない。
CI は固定評価 workload 上の training seed 変動であり、未知 workload 母集団の不確実性を含まない。

## Counterfactual

同一 task features を持つ全ての異なる held-out block の ordered pair を使用する。
報酬差・oracle action・workload label で pair を選別しない。
A の過去履歴で得た hidden を clone し、最後の body だけ A/B に分ける。
B の実測 cost matrix で、body 対応 action と A に固定した action を比較する。
action 変化率に加え、utility gain と latency 差を評価する。

これはモデル内部の body 入力介入と、保存済み実測 job cost による replay。
物理的な body 状態そのものの無作為化介入ではない。
実行した policy choice による live job の再検証は別 artifact として記録する。

## OOD

Temporal: sampling interval 2 倍、2 観測遅延、jitter、30% dropout、実測 stale 30 秒、履歴長変更。
Sensor: Gaussian noise SD0.1、30% dropout、定数0.5、実測 stale、1.5倍scale、RTX sensor欠損、RTX資源不在の模擬、Mac欠損、RTT最大、CPU/GPU busy反転、固定sensor permutation。

いずれも controlled observation perturbation。報酬は同じ実測 cost matrix を使う。
network/thermal/GPU の実機障害を作っていない。
`rtx3060_sensor_unavailable` は RTX センサー欠損で、コスト・failure は元の実測値を保持する。
別条件 `controlled_resource_unavailable` は同じ欠損観測に加え、RTX選択時だけ utility=-1、failure=1 とする資源障害の simulation。CPU/P100 の結果は実測値を保持する。物理 GPU は停止していない。
sensor permutation 等で崩壊した場合も mode ごとの数値を残す。

## 出力

`training_config.json`, `run_summary.json`, `runs/*/{metrics.jsonl,status.json,best.pt,final.pt}`、
`prediction_probe.json`, `prediction_probe_models.json`, `prediction_probe_predictions.jsonl`、
`ablation_results.json`, `counterfactual_body.json`, `counterfactual_pairs.jsonl`、
`ood_results.json`, `policy_traces.jsonl`, `policy_input_traces.jsonl` を保存する。
policy traces は action、hidden norm、元 body、実測 latency/utility、provenance を持つ。
policy input traces は seed/mode ごとに介入後の実際の 44float sequence を保存する。
チェックポイントのバイナリは raw data と同様、無条件に Git へ追加しない。
