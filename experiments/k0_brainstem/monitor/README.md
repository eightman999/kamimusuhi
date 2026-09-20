# K0 monitoring

FastAPI server は `127.0.0.1:8097` のみに bind する。trainer は別 session の process として動作し、GUI/server の終了では停止しない。PAUSE/RESUME/STOP は既知 run の `control.json` に要求を書き、trainer が update 境界で受け付ける。STOP は checkpoint を保存する。

```bash
# Remote host: repository root, training venv
.venv-k0/bin/python -m experiments.k0_brainstem.monitor.server --artifacts experiments/k0_brainstem/artifacts

# Mac: isolated monitor venv (PyTorch is unnecessary)
python3 -m venv .venv-k0-monitor
.venv-k0-monitor/bin/python -m pip install -r experiments/k0_brainstem/requirements-monitor.txt
K0_SSH_HOST=your-ssh-alias ./run_k0_monitor.command
```

接続先は `K0_SSH_HOST`、または `.local/connections/k0-ssh-host` の1行で指定する（環境変数を優先）。`.local/connections/` は `.gitignore` で除外している。ホスト名・IP・ユーザー名・鍵の参照先などの接続情報はこの配下に保存し、秘密鍵・トークンそのものはコピーしない。

既存 SSH tunnel を使う場合は `K0_NO_TUNNEL=1 K0_MONITOR_PORT=18097 ./run_k0_monitor.command`。直接接続先を指定する場合は `.venv-k0-monitor/bin/python -m experiments.k0_brainstem.monitor.mac_gui --server http://127.0.0.1:18097`。

GUI は HTTP と WebSocket を使う thin client。再接続は 1–30 秒の backoff で自動試行し、再接続時に履歴を取得する。6 指標の graph、run 表、GPU/CPU/RAM、action 分布、内部 state、evaluation episode、language gate を表示する。START は configs 下の YAML のみを受け付け、任意 command/path は実行しない。既存 compute process が使う GPU および queue 実行中の追加 START は拒否する。GPU UUID と物理 index を記録し、異なる GPU の `cuda:0` を区別する。

## 検証記録

`tests/test_monitor.py`: 9 tests PASS。SQLite WAL 並行読み取り、制御ファイル、path/command 制限、browser Origin 拒否、GPU 二重起動防止、queue 優先、episode/language 読み取りを検証。Qt テストは実際のローカル server を起動して WebSocket 接続、server 終了・再起動後の自動再接続、STOP 確認の取消、GUI 終了時の server 継続を確認した。Qt が未導入の環境では GUI テストのみ SKIP。

`../artifacts/gui_smoke.png` は fixture ではなく、Mac の通常 GUI が SSH tunnel `127.0.0.1:18097` 経由でリモートの実 GPU smoke 結果を表示した screenshot。`gui_smoke_verification.json` は実測確認結果。

確認した表示は 3 runs、`smoke-gru64` の 4 metrics、RTX 3060 と P100 の温度・VRAM・利用率。実際の GUI WebSocket 切断から自動再接続まで成功し、制御要求は送信していない。取得時には評価前のため実 episode は 0 行。episode/language viewer の描画はこの段階では fixture による確認であり、実 J72 evaluation の表示証拠とは区別する。学習 process の server 停止後の生存、および実 GPU の PAUSE/RESUME は別の smoke 証拠を参照する。

## 日本語表示

見出し、操作ボタン、実験状態、計測グラフ、表示期間、内部状態の指標名、エピソード行動、言語ゲート、エラー案内、停止確認を日本語表示にした。実験ID・構成名・設定ファイル名とAPIのJSONキー／制御コマンドは保持する。J72の生成応答は原文を表示する。

macOSでは標準の `Hiragino Sans` を優先する。offscreen描画と主要な日本語文字のglyph存在を確認済み。表示変更後のmonitor testは11件PASSで、停止ダイアログの日本語ボタン・取消、表示を日本語にしても送信プロトコルが変わらないことを含む。
