# 個体別ニューロン活動表示 v0

MIOBA Observatoryの個体インスペクターで、**主評価の実行中に取得したニューロン別の発火回数**を表示する。対象はTorchまたはMock backendによる神経シミュレーションであり、optimizerによる学習過程や重み更新を観測する機能ではない。

## 表示するもの

- ニューロンのインデックス順に並べた格子。解剖学的な座標や脳部位の配置を表すものではない。
- 各セルは観測窓内の発火回数を表す。基盤ニューロンと人工ニューロンを色で区別し、発火が0のセルは暗く表示する。
- 母集団全体から決定的な等間隔インデックスを選ぶ。既定は最大256個、設定上限は512個。総数が指定数以下なら全ニューロンを表示する。
- 表示対象は現在のexecution batchの**lane 0**。そのlaneをglobal `replicate_index`と`replicate_seed`へ対応付け、現在batchの代表replicateとして表示する。全replicateの平均ではない。
- 実験・個体・job・worker、attempt、evaluation pass、sequence、受信時刻、simulation時間、観測窓、受信からの経過時間を併記する。
- ブラウザー上の短い履歴は、表示サンプルのうち窓内に1回以上発火したニューロンの割合を保持する。既存評価summaryの`active_fraction`（発火したreplicateの割合）とは異なる。

格子は実測countで更新する。観測欠損を補うための架空の点滅や、完全なspike trainの再生は行わない。Mock由来の表示にはMockであることを明記する。

## 取得と実行の分離

TorchとMockが既に保持する`spike_counts`から、選択ニューロン・代表laneの累積countをcopyする。前回countとの差分と、実際の`backend.t_ms`の差で観測窓を定める。追加の乱数、入力、神経stepは発生させない。

観測は既存の実行区切りで行う。

| 項目 | v0の動作 |
|---|---|
| 環境なしの評価 | 既存の最大50 simulation msのchunk直後 |
| 環境ありの評価 | 既存の環境slice直後 |
| 通常の取得間隔 | wall時間で約0.5秒以上。区切りが来た時点で取得する |
| batch終了 | 最後の実観測窓を送る。同じsimulation時刻の重複frameは作らない |
| batch切替 | 初期化後のcountと時刻を新しい窓の起点にする |
| 同attempt内のOOM再評価 | `evaluation_pass`を0、1、…と増やす |
| sequence | 同じjob実行（attempt）内で単調増加し、passやbatch切替でも戻さない |
| 想定外の時刻巻き戻り・count減少 | そのbatchの観測を停止して診断を記録する。0への丸めや暗黙の再基準化はしない |

観測対象は`phase: evaluation`の主評価だけで、機能欠損・ablation等の追加評価にはobserverを渡さない。受信したframeはjobの成功や個体の能力を証明するものではない。

送信は別スレッドで行う。待機queueは2件までとし、古いframeを新しいものに置き換える。HTTP timeoutは0.3秒、終了時は最新の待機frameだけをflushし、終了待機を最大1秒に制限する。通常のHTTPクライアントがtimeoutを守ることを前提とする。ネットワークエラーや観測エラーを神経評価の失敗へ伝播させない。

## APIと鮮度

| API | 用途 |
|---|---|
| `POST /api/worker/neural-activity` | workerから観測frameを送信 |
| `GET /api/gui/individual/{genome_id}/neural-activity` | 個体の最新frameと鮮度・job状態を取得 |

送信frameのschema versionは1。識別情報として`experiment_id`、`genome_id`、`job_id`、`worker_id`、`attempt`、`evaluation_pass`、`sequence`、`backend`、`phase`、`replicate_index`、`replicate_seed`を持つ。観測値は`n_neurons`、`n_base`、`neuron_indices`、窓内差分の`spike_counts`、`window_start_ms`、`window_end_ms`に限定する。

coordinatorは型・範囲・件数を検査し、実験と個体の所属、RUNNING jobの所有workerとattempt、backend、replicate seedを照合する。同じpass・replicateの時間逆行、ニューロン数・サンプル変更、sequenceの重複・逆行も拒否する。受信時刻はcoordinatorが付け、鮮度はcoordinatorのmonotonic clockで測る。

ブラウザーは約750ms間隔で取得する。TTLは5秒で、次の状態を区別する。

| 状態 | 意味 |
|---|---|
| `LIVE / running` | jobがRUNNINGで、最新frameの受信から5秒以内 |
| `RECORDED / stale` | RUNNINGだがframeが古い、またはjobがUNKNOWN等で終了未確認。現在も発火しているとは表示しない |
| `RECORDED / recorded` | job終了後に残った最終frame。終了時点の観測として表示 |
| `UNAVAILABLE / waiting` | 評価待ち・実行中だがframe未受信 |
| `UNAVAILABLE / unavailable` | 利用できるframeがない |

接続失敗時も最後の観測を現在の活動として点灯させない。同じframeを再取得しても、鮮度や履歴を更新したことにはしない。

## 保存範囲

coordinatorは**最大128 job、各jobの最新1 frameだけ**をプロセスメモリに保持する。古いjobは容量上限で追い出す。TTLはLIVE判定の期限であり、5秒経過したframeを即削除する設定ではない。

この機能のframeはSQLite、評価summary、genome、fitness、実験の履歴へ書き込まない。coordinatorの再起動で失われる。ブラウザーのサンプル活動率履歴も最大48点の一時表示で、個体・job・attempt・pass・代表replicateが切り替わるとリセットする。

## 有効化・無効化

通常のworker CLIでは既定で有効。既存の起動コマンドに次を加えると無効にできる。

```text
--no-neural-activity
```

表示サンプル数は次で変更する。許容範囲は1〜512。

```text
--neural-activity-sample-size 256
```

テスト等で`run_job`を直接呼ぶ場合は、既存呼び出しへの影響を避けるため既定で無効。`neural_activity=True`で有効にし、必要なら`neural_activity_sample_size`を渡す。

## 検証済みの範囲と残件

2026-09-15の最終確認では、API・worker・GUIの新規試験と関連回帰を合わせて**324件PASS**。既存依存の警告4件あり。

- Torch CPU／Mock、環境あり／なしで、観測ON/OFFの神経checkpoint、乱数状態、発火、科学的summary、選択指標が一致した。
- getterの返却値を変更してもbackendへ影響しないこと、batchとglobal replicateの対応、観測窓、OOM再評価とsequenceを確認した。
- 通信失敗・queue上書き・最終flush・送信スレッド終了を確認した。
- 個体・job・attempt・replicateの分離、旧frame拒否、TTL、64 KiB上限、DB不変を確認した。
- GUIの実JavaScriptをNodeで実行し、画面切替後の遅延応答、重複frame、再試行時の消去、取得失敗時のライブ停止を確認した。

### 実worker・ブラウザー確認

既存の研究runとは別の一時runs-dir・空きportで、通常CLIのcoordinatorとworkerを接続した。テスト用Pythonは既存のリポジトリ`.venv`を使用した。

- Mock worker 2台で8個体を評価し、全個体の観測frameをGET APIから取得した。最終版でもfresh runで再確認した。
- Torch CPU worker 2台で、合成128ニューロンの基盤個体と人工54ニューロンを追加した個体を同時評価した。各60,000 simulation msを完了し、個体別に106件・113件の異なるframeを受信した。送信の最終sequenceは106・114で、一部窓が観測から抜けても補間していない。
- ブラウザーでTorchの`LIVE`表示、フレーム番号・発火窓・格子・活動履歴の更新を確認した。取得済みの評価summaryをライブ表示に流用していない。
- 評価終了後の記録表示、接続終了後のライブ表示停止、最終版で記録時にも発火色を保持することを確認した。
- 幅400pxでページ幅400px、格子・metadata幅350pxを確認した。横方向のはみ出しなし。

この実接続確認は合成ネットワークのCPU実行であり、実FlyWireデータやGPUの検証ではない。一時coordinator・worker・ブラウザーは停止済みで、使用したプロセスとportの残存がないことも確認した。

観測copy・同期・queue処理には実費があるため、wall時間、throughput、timingの完全な同一性は主張しない。既存の`event_propagation_seconds`もwall計測値であり、科学的同一性の比較から分けている。

GeNNは未対応。この神経活動表示のGPU上での非干渉性・負荷は未検証であり、既存MIOBAのGPU検証結果から補完しない。

### 検証入口

```bash
.venv/bin/python -m pytest -q \
  experiments/mioba/tests/test_neural_activity_api.py \
  experiments/mioba/tests/test_neural_activity_worker.py \
  experiments/mioba/tests/test_neural_activity_gui.py \
  experiments/mioba/tests/test_backend.py \
  experiments/mioba/tests/test_torch_backend.py \
  experiments/mioba/tests/test_heterogeneous.py \
  experiments/mioba/tests/test_m1_environment.py \
  experiments/mioba/tests/test_coordinator.py \
  experiments/mioba/tests/test_gui.py \
  experiments/mioba/tests/test_gui_i18n.py \
  experiments/mioba/tests/test_m1_observatory.py \
  experiments/mioba/tests/test_dialogue_snapshot.py
```

## 実装参照

- [workerの観測・送信](../../experiments/mioba/workers/neural_activity.py)
- [workerの評価経路](../../experiments/mioba/workers/worker.py)
- [環境episode](../../experiments/mioba/mie/episode.py)
- [backend共通境界](../../experiments/mioba/fba/backend.py)
- [Torch getter](../../experiments/mioba/fba/torch_backend.py)
- [Mock getter](../../experiments/mioba/fba/mock_backend.py)
- [coordinatorの検査・一時cache](../../experiments/mioba/gui/neural_activity.py)
- [API routes](../../experiments/mioba/coordinator/app.py)
- [Observatory表示](../../experiments/mioba/gui/static/index.html)
