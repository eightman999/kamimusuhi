---
name: mioba-observatory-browser-smoke
description: MIOBA Observatoryの日本語GUIをmock workerでブラウザー検証する際の環境分離、fixture、撮影、後片付け。
---

# MIOBA Observatory browser smoke

- 既存`$HOME/mioba-venv`と`experiments/mioba/configs/smoke_mock.yaml`を使用する。
- coordinatorは固有のruns-dirと空いているportを選ぶ。他のsmoke suiteと同じportを同時使用しない。workerが意図しないcoordinatorに登録され、画面データが混在する場合がある。
- mock workerを2台、異なるworker-id、`--backend mock --execution-batch 1 --heartbeat-s 1`で起動する。短いconfigは8個体を評価後に待機する。
- UIの初期言語はja。日本語が四角に見える場合はCJKフォントを入れ、font cacheを更新してブラウザーを再起動する。
- ナビは`#/dashboard`、`#/population`、`#/organism/<genome_id>`、`#/telemetry`、`#/events`。一覧の変異数と詳細の累積パラメータ変異数を同一IDで比較する。
- 器官が必要なら一時configで`evolution.mutation_seed: 1`、`population.target_size: 4`、`evolution.max_generations: 0`を使う。設定構造は現行smoke configを参照し、別runで生成する。
- 実GPUがない場合はGPU性能検証と主張しない。GPUカード用の架空identityをAPIで注入するのはユーザーが明示的に許可した場合のみ。heartbeatを継続しonline表示を維持する。
- 全ページ撮影はChrome CDPの`Page.captureScreenshot`が使える。CDPポートは実際のChrome起動引数から確認し、固定の9222と仮定しない。`websocket-client`は任意の撮影補助依存。
- 狭幅は400pxでカード縦並びを目視し、ページのclientWidthとscrollWidthを比較する。表内のスクロールとページ全体のoverflowを区別する。
- coordinator/backendを変更したら再起動する。同じ実験を復元するには`start --resume <experiment_id>`を使う。新規スクリーンショットが必要なら混在していないfresh runを使う。
- 終了時はCLIのグローバル`--experiment-id`で対象を指定して`stop`。coordinatorは終了する。workerとheartbeat loopも別途終了し、対象portのlistenerとプロセスが残っていないことを確認する。

## Devin Secrets Needed

ローカルmock検証はなし。control API用tokenはcoordinatorが生成しCLIがrunディレクトリから読む。
