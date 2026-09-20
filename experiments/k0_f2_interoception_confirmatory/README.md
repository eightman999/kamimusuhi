# K0-F2 Machine Interoception Confirmatory Study

K0-Fのコードfamilyを新しいnamespaceへ分離した確認実験。旧K0-Fのraw、validation、test、正規化fit、checkpoint、結果は再利用しない。研究上の正典は `protocol_spec.json` と、収集前にcommitされた `protocol_lock.json` / `F2_PROTOCOL_LOCK`。結果を見てgate、seed、task、λ、architecture、学習量を変更しない。

## 実行順序と停止境界

1. 実装sourceをcommitし、そのSHAと科学コードhashを含むlockを別commitに固定する。`F2_PROTOCOL_LOCK` はlock commitを指す。lockの検証とreceipt保存後にfresh収集を開始する。
2. 新規session A/Bをtrain、Cをvalidationとして取得する。各16block × 32decision、4task × 8履歴長。row単位のrandom splitはしない。安全停止や無効な収集を隠して再試行しない。
3. 固定ridge λ10でaction-conditioned log latency / utilityを予測する。validationの両targetでBODY MAEがBLIND・SHUFFLEDより小さく、BODY selection regretも両対照より小さい場合だけgate PASS。
4. **gate FAILなら `FAIL_AT_PROBE_GATE` で終了する。D/test収集・読み出し、policy学習、live、探索的な継続は行わない。** 未実施artifactは `NOT_RUN_PROBE_GATE_FAILED` と空rowsを保存する。描画やreportでは未実施を成功値へ置換しない。
5. PASS時のみGRU128 BODYと独立学習BLINDを固定12seed（0–11）、240epochで学習。同初期重み・task順を使用し、validation utilityだけでbestを選択する。best/finalを保存し、全checkpointを凍結する。
6. 凍結後に新規D/test 16blockを取得し、P1（独立BLIND）、P2（SHUFFLED）、P3（30秒STALE）、counterfactual、固定OODを評価する。masked BODY-checkpoint BLINDは診断として別記する。
7. 新規live L1/L2各8blockで凍結policyを実行する。12seed × 4task × 4modeで計3072job。P4は新規実ジョブのBODY対独立学習BLIND。保存cost replayをlive実測と呼ばない。
8. 独立監査、安全・cleanupを確認し、日本語27節reportと12図を保存する。P1–P4とCFは固定12seed、平均utility差0.01以上、seed bootstrap 95% CI下限>0、exact sign p<=.05を全て要求する。部分成立で研究PASSにしない。次phaseやpushは自動実行しない。

4actionの順序は `RUN_CPU`, `RUN_RTX3060`, `RUN_P100`, `WAIT`。WAITは20ms待機後に同じjobをRTX3060で実行する。再観測・再計画はしない。latencyは待機を含むcompute completionまでを測り、結果検証・返送を含めない。全行列finiteとfirst-row独立checksumを検証するが、全要素一致の証拠とは呼ばない。

## コマンド

以下はrepository rootから実行する。dataset、lock、実行先の配置を確定させた後のレシピであり、このREADMEの作成自体は収集やfitを実行しない。Python環境には各CLIがimportするtorch/numpy/psutil等が必要。GUIはPyQt5/pyqtgraph、図はmatplotlib/numpyを使う。

接続値はgitignore済み `.local/connections/` 内のprivate設定から運用側で読む。公開ファイルへSSHの実ホスト、user、鍵、tokenを転記しない。以下の `F2_*` 変数はそのprivate設定と今回の実行ディレクトリを参照する。

```sh
python -c 'from experiments.k0_f2_interoception_confirmatory.protocol import load_lock; import sys; load_lock(sys.argv[1]); print("protocol lock verified")' "$F2_LOCK"

python -m experiments.k0_f2_interoception_confirmatory.run_collection \
  --ssh-alias "$F2_SSH_ALIAS" --peer-host "$F2_PEER_HOST" \
  --remote-root "$F2_REMOTE_ROOT" --remote-python "$F2_REMOTE_PYTHON" \
  --sensor-binary "$F2_SENSOR_BINARY" --output "$F2_LOCAL_SESSION_OUTPUT" \
  --remote-output "$F2_REMOTE_SESSION_OUTPUT" --phase record \
  --session-id "$F2_SESSION_ID" --study-root "$F2_REMOTE_STUDY_ROOT" \
  --protocol-lock "$F2_REMOTE_LOCK" --blocks 16 --block-seconds 45
```

`F2_SESSION_ID` は順にA/B/C。各sessionに未使用output `artifacts/collection/A` 等を用意する。Dはgate通過・全policy凍結後だけ実行する。study rootを `artifacts` とし、dataset builderがmanifest/hashを保存する。A/Bは `train/dataset.jsonl`、Cは `validation/dataset.jsonl`、gate・freeze後のDは `test/dataset.jsonl` へ構成する。

```sh
python -m experiments.k0_f2_interoception_confirmatory.build_dataset \
  --artifacts "$F2_ARTIFACTS" --protocol-lock "$F2_LOCK"
```

`F2_TRAIN_DATASET="$F2_ARTIFACTS/train/dataset.jsonl"`、`F2_VALIDATION_DATASET="$F2_ARTIFACTS/validation/dataset.jsonl"`、`F2_PROBE_OUTPUT="$F2_ARTIFACTS/validation"`、`F2_POLICY_OUTPUT="$F2_ARTIFACTS/train"` とする。D取得後だけ同builderに `--include-test` を追加する。Dのcollectorとbuilderはstudy rootの `validation/probe_validation.json` と `train/checkpoint_manifest.json` を検証する。

```sh
python -m experiments.k0_f2_interoception_confirmatory.probe \
  --train-dataset "$F2_TRAIN_DATASET" --validation-dataset "$F2_VALIDATION_DATASET" \
  --protocol-lock "$F2_LOCK" --output "$F2_PROBE_OUTPUT"
```

probe outputは新規session Cの `validation` ディレクトリを指定し、同じ場所の `dataset.jsonl` を許容する。既存probeの上書きや再fitはしない。gateの成否は `probe_validation.json` のstatusとgateで判断する。CLI終了コードだけをgate PASSと見なさない。FAIL時はここからcleanup・未実施reportへ進み、次のコマンドは実行しない。

```sh
# probe PASS時のみ。trainにはtestパスを渡さない。
python -m experiments.k0_f2_interoception_confirmatory.policy --stage train \
  --train-dataset "$F2_TRAIN_DATASET" --validation-dataset "$F2_VALIDATION_DATASET" \
  --probe "$F2_PROBE_OUTPUT/probe_validation.json" --protocol-lock "$F2_LOCK" \
  --output "$F2_POLICY_OUTPUT" --device cpu --threads 4

# 全policy凍結後に新規Dを取得し、そのdatasetだけを指定する。
python -m experiments.k0_f2_interoception_confirmatory.policy --stage evaluate \
  --train-dataset "$F2_TRAIN_DATASET" --validation-dataset "$F2_VALIDATION_DATASET" \
  --test-dataset "$F2_TEST_DATASET" --probe "$F2_PROBE_OUTPUT/probe_validation.json" \
  --protocol-lock "$F2_LOCK" --output "$F2_POLICY_OUTPUT" --device cpu --threads 4

# gate通過・全checkpoint凍結・D評価後。L1/L2ごとに新規outputを指定する。
python -m experiments.k0_f2_interoception_confirmatory.run_collection \
  --ssh-alias "$F2_SSH_ALIAS" --peer-host "$F2_PEER_HOST" \
  --remote-root "$F2_REMOTE_ROOT" --remote-python "$F2_REMOTE_PYTHON" \
  --sensor-binary "$F2_SENSOR_BINARY" --output "$F2_LOCAL_SESSION_OUTPUT" \
  --remote-output "$F2_REMOTE_SESSION_OUTPUT" --phase live \
  --session-id "$F2_SESSION_ID" --study-root "$F2_REMOTE_STUDY_ROOT" \
  --protocol-lock "$F2_REMOTE_LOCK" --training-artifacts "$F2_REMOTE_POLICY_OUTPUT" \
  --dataset "$F2_REMOTE_TEST_DATASET" --blocks 8 --block-seconds 45
```

## 日本語GUI・図・report

```sh
./run_k0_f2_gui.command
python -m experiments.k0_f2_interoception_confirmatory.gui --artifacts "$F2_REVIEW"
python -m experiments.k0_f2_interoception_confirmatory.visualize --artifacts "$F2_REVIEW"
python -m experiments.k0_f2_interoception_confirmatory.report --artifacts "$F2_REVIEW"
```

launcherの既定artifactは `experiments/k0_f2_interoception_confirmatory/artifacts/review`。`K0F2_ARTIFACTS` と `K0F2_GUI_PYTHON` で上書きできる。GUIは読み取り専用。保存済みtimestamp・現在からの経過時間・実runtimeを区別し、欠損を正常値0にしない。`current_decision.json` からtask、BODY/BLIND選択、4action予測utility、実資源・実測latency・hiddenを表示する。record-onlyではpolicy/probe未実施と表示する。

reviewにはraw Mac/master、aligned telemetry、20次元frame、protocol/collection/split/normalization、probe、独立監査、resource summary、最終runtimeを配置する。後段のtest/live/checkpoint/traceはgate PASS時のみ配置する。GUI・図・reportは失敗または未判定gateから後段結果ファイルを開かない。

図は `body_timeseries`, `action_conditioned_probe`, `probe_regret`, `body_vs_blind`, `body_vs_shuffled`, `body_vs_stale`, `live_body_vs_blind`, `counterfactual_body`, `sensor_ood_heatmap`, `temporal_ood`, `policy_action_map`, `pareto` の12 PNG。`plot_manifest.json` に出典hash・状態・図hashを保存する。科学図のCIはseed単位で計算し、1Hzのsensor sampleを独立nとしない。未実施図は理由を記す。reportは `K0_F2_REPORT.md`, `report_statistics.json`, `success_criteria.json` を生成する。

## 停止と検証

Mac側の有限collectorはCtrl-C/SIGTERMでowned remote driverにSTOPを要求し、sensor source、受信SSH、network workload、caffeinateを終了する。remote session outputに `STOP` ファイルを作る方法もある。保存データは削除しない。停止後に `transport_state.json` と `collection_runtime_state.json`、GPU compute PID、温度、CPU/RAM、Mac状態を照合する。既存serviceは開始前の状態へ戻し、temporary mirror/tunnel/socketを閉じる。GUIを閉じる操作だけでcollector停止と判断しない。

```sh
python -m unittest experiments.k0_f2_interoception_confirmatory.tests.test_report -v
QT_QPA_PLATFORM=offscreen python -m unittest experiments.k0_f2_interoception_confirmatory.tests.test_gui -v
```

fixtureテスト・offscreen描画は表示ロジックの確認であり、fresh実取得、モデル性能、実GUI画面、新規live hardwareの証拠ではない。実artifactの再生成とnative GUI確認は収集完了後に別記する。
