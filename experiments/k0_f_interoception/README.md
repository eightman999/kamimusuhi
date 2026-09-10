# K0-F 機械的内受容

Mac と中央計算ノードの実 telemetry を20次元の身体量へ正規化し、GRU128による CPU / RTX3060 / P100 の資源選択を検証する独立実験。研究質問は「現在の正しい身体情報が下流効用を改善するか」であり、telemetry取得やGUIの表示だけを成功と呼ばない。

正典は [PROTOCOL.md](PROTOCOL.md)、sensor契約は [SENSOR_CONTRACT.md](SENSOR_CONTRACT.md)、学習・評価契約は [POLICY_CONTRACT.md](POLICY_CONTRACT.md)。K0-E/E2を変更せず、Git commitのみ許可、外部pushは行わない。

## 証拠を分ける

| 層 | 記録 | 意味 |
| --- | --- | --- |
| 実sensor | raw_mac / raw_master / aligned / frames | 実機の観測。欠損はnullとquality/mask0 |
| 保存実測費用の再生 | ablation / counterfactual / OOD | 選択actionを保存済み実測costで採点 |
| 新規実ジョブ | live_jobs / live_results | 凍結方策が選んだ資源で新たに実行し実測 |
| 合成介入 | OOD kind / mode | 観測変更またはRTX不在simulation。実故障ではない |

入力は task4 + masked body20 + mask20 = 44 float。workload名、絶対時刻、episode ID、正解costは入力しない。出力の整数は `0=RUN_CPU, 1=RUN_RTX3060, 2=RUN_P100`。

## 環境とprivate接続

Mac GUIは既存PyQt5/pyqtgraph環境を使う。収集・学習側はPython、NumPy、PyTorch、図生成はMatplotlibを使用する。Mac sensorはSwiftの公開APIから取得し、root権限や温度helperを導入しない。

接続情報はGit除外済み `.local/connections/llm-master.json` と `.local/connections/k0-ssh-host` に保存した既存設定から参照する。ホスト名、IP、user、秘密鍵、tokenをREADME・raw・config・成果物に転記しない。下記の `K0F_*` はそのprivate設定を使う実行環境で設定する変数であり、実値をリポジトリへ保存しない。

```sh
git check-ignore .local/connections/llm-master.json .local/connections/k0-ssh-host
mkdir -p .local/bin
swiftc -O experiments/k0_f_interoception/mac/k_sense_mac.swift -o .local/bin/k-sense-mac
.local/bin/k-sense-mac --samples 5 --interval 1 --output .local/mac_sensor_smoke.jsonl
```

`K0F_SSH_ALIAS`, `K0F_PEER_HOST`, `K0F_REMOTE_ROOT`, `K0F_REMOTE_PYTHON`, `K0F_REMOTE_ARTIFACTS` をprivate設定から設定する。`K0F_LOCAL_ARTIFACTS` はMac transportの保存先。以下は新しい空ディレクトリにだけ実行する。既存完了結果へ追記しない。

## A: 記録のみ

```sh
python -m experiments.k0_f_interoception.run_collection \
  --ssh-alias "$K0F_SSH_ALIAS" --peer-host "$K0F_PEER_HOST" \
  --remote-root "$K0F_REMOTE_ROOT" --remote-python "$K0F_REMOTE_PYTHON" \
  --sensor-binary "$PWD/.local/bin/k-sense-mac" \
  --output "$K0F_LOCAL_ARTIFACTS" --remote-output "$K0F_REMOTE_ARTIFACTS" \
  --blocks 48 --block-seconds 20 --source-commit "$(git rev-parse HEAD)"
```

Mac側driverはSwift sensorのraw sourceを保持し、SSH receiverが中央受信時刻を付ける。中央driverは常に有限時間で動作し、終了時は所有した背景処理・sensorを停止する。作業中のMac sleepを避けるため、driver自身が期間限定のidle-sleep抑制を所有して終了時に解除する。OSの永続電源設定は変更しない。

初回primary収集中にはMac sleepによる壁時計約805秒・monotonic約1秒の不連続が生じ、安全停止した記録がある。これを保持し、同じprotocolで独立ディレクトリ `primary_v2` に再取得する。初回中断データを完了runや独立seedに混ぜない。実行状態・件数は各artifactのmanifestで確認する。

splitはworkload block単位 train24 / validation8 / test16。35秒境界bufferを入れ、task測定は各block8decision。保存bodyから長さ1～8を各block一度ずつ、costと成績を使わず割り当てる。

```sh
python -m experiments.k0_f_interoception.prepare_dataset \
  --source "$K0F_DATASET_RAW" --frames "$K0F_FRAMES" --output "$K0F_DATASET"
```

record-only実LLM生成は `auxiliary llm` の専用有限区間を使う。既存のローカルモデル重み・loaderをprivate設定で指定する。行列乗算の負荷をLLM推論実証として扱わない。

## B: 予測価値のgate

```sh
python -m experiments.k0_f_interoception.probe \
  --dataset "$K0F_DATASET" --output "$K0F_POLICY_ARTIFACTS"
```

保存 `prediction_probe.json` の `gate.pass` がtrueの場合だけpolicyへ進む。train-only ridge/scaling、validationのBODY正規化MAEがBLINDより10%以上低いことを事前条件とする。未達ならtestを開かず停止理由を報告する。sensor再設計は旧protocol・旧pilotを保全し、別namespaceへ記録する。

## C: 教師学習、固定checkpoint、評価

```sh
python -m experiments.k0_f_interoception.policy \
  --dataset "$K0F_DATASET" --output "$K0F_POLICY_ARTIFACTS" \
  --probe "$K0F_POLICY_ARTIFACTS/prediction_probe.json" \
  --normalization-config "$K0F_NORMALIZATION" --stage train \
  --seeds 0,1,2,3,4,5,6,7 --hidden-sizes 128 \
  --epochs 160 --batch-size 64 --learning-rate 0.001 --threads 4
python -m experiments.k0_f_interoception.probe \
  --dataset "$K0F_DATASET" --output "$K0F_POLICY_ARTIFACTS" --include-test
python -m experiments.k0_f_interoception.policy \
  --dataset "$K0F_DATASET" --output "$K0F_POLICY_ARTIFACTS" \
  --probe "$K0F_POLICY_ARTIFACTS/prediction_probe.json" \
  --normalization-config "$K0F_NORMALIZATION" --stage evaluate \
  --seeds 0,1,2,3,4,5,6,7 --hidden-sizes 128 \
  --epochs 160 --batch-size 64 --learning-rate 0.001 --threads 4
```

16run = GRU128 BODY/BLIND各8seed。BODY-bestへのBODY/BLIND/SHUFFLED/STALEをprimary、独立学習BLINDをsecondaryとする。bestはvalidation最大、finalは別結果に保存。PPO・DAggerを自動追加しない。中断runの上書きは拒否し、完了runはsource/data/normalization/checkpoint hash一致時だけ再利用する。

## 凍結Coreによる新規実ジョブ

同じprivate変数を使い、出力には新しいliveディレクトリを指定する。8seed×4task×5mode×8workload blockで実行順を無作為化し、selected resourceで新規jobを実行する。

```sh
python -m experiments.k0_f_interoception.run_collection \
  --phase live --ssh-alias "$K0F_SSH_ALIAS" --peer-host "$K0F_PEER_HOST" \
  --remote-root "$K0F_REMOTE_ROOT" --remote-python "$K0F_REMOTE_PYTHON" \
  --sensor-binary "$PWD/.local/bin/k-sense-mac" \
  --output "$K0F_LOCAL_LIVE" --remote-output "$K0F_REMOTE_LIVE" \
  --training-artifacts "$K0F_REMOTE_POLICY_ARTIFACTS" --dataset "$K0F_REMOTE_DATASET" \
  --blocks 8 --block-seconds 240 --source-commit "$(git rev-parse HEAD)"
```

live BODY対BLINDとBODY対独立学習BLINDを主確認とし、replay成功をlive成功で代用しない。shared physical workloadと順次実行の制約を報告する。

## 日本語GUI・図・report

```sh
K0F_GUI_PYTHON="$K0F_GUI_ENV/bin/python" K0F_ARTIFACTS="$K0F_PRIMARY" ./run_k0_f_gui.command
python -m experiments.k0_f_interoception.visualize --artifacts "$K0F_PRIMARY"
python -m experiments.k0_f_interoception.report --artifacts "$K0F_PRIMARY" \
  --policy-artifacts "$K0F_POLICY_ARTIFACTS" --live-artifacts "$K0F_LIVE"
```

GUIは保存フォルダーを2秒ごとに読む閲覧専用。中央hostのcontrolや負荷起動は行わない。生値・normalization・欠損・age・qualityを表示し、live_jobsがある場合は新規実ジョブのactionを優先表示する。再生評価のtraceはliveと区別する。図生成は10PNGとinput/image hash manifestを作り、欠損panelは未取得を明示する。統計上のnはseedでありtelemetry点数ではない。

reportは24節の日本語Markdownと `report_statistics.json` / `success_criteria.json` を生成する。最終監査の `resource_summary.json` は `telemetry_continuous`, `fixed_frame_valid`, `safety_pass`, `reproducibility_pass`, `execution_complete`、runtimeは `cleanup_pass`、baseline監査は `pass` を保存する。未知・欠損・falseは成功へ昇格しない。liveの2主比較を含む全gateを要求する。

## 検証と終了

```sh
python -m unittest discover -s experiments/k0_f_interoception/tests -p 'test_*.py'
QT_QPA_PLATFORM=offscreen "$K0F_GUI_ENV/bin/python" -m unittest experiments.k0_f_interoception.tests.test_gui
```

GUI以外の環境では `test_gui.py` を別のPyQt5環境で実行する。GUIの存在、offscreen描画、native画面確認、実sensor取得、実ジョブ実行を別の検証層として記録する。

通常終了とCtrl-C/SIGTERMはdriverのfinally経由でowned sensor、workload、transport、期間限定sleep抑制を終了する。中断時はdriver_identityのPID・owner・実commandを照合した今回のdriverだけを終了し、未知processをkillしない。通信切断時はsensor/workloadの有限durationと中央runtime artifactを確認してから再接続する。

終了条件はtraining worker、temporary workload、Mac/master sensor、transport停止、GPU compute PID確認、thermal/load通常範囲への復帰、今回のscratch/socket消失、既存service状態一致。`final_runtime_state.json` と `baseline_integrity.json` を保存する。ログ・既存checkpoint・中断記録を削除しない。commit一覧を最終結果に付け、pushせず、次phaseの承認を最後に一度だけ求める。
