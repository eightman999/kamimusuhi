# モデル利用監視

Grafanaの `Kamimusuhi · モデル利用監視` はPrometheusからresidentのモデル利用を表示する。
`generate_dashboard.py` で `kamimusuhi-model-usage.json` を再生成できる。
既定データソースは現在のPiの `prometheus`。他環境では画面上部のデータソースを変更する。

## 収集対象

- `/metrics`: Prometheusテキスト形式。モデル/経路/成功失敗別の呼び出し数、latency histogram、tokens、料金、最終時刻。
- `/v1/usage`: 起動後の集計JSONと永続保存状態。
- `/v1/usage/history?limit=100&since=0`: 新しい順の呼び出し履歴。limitは1〜1000、sinceはUnix秒。
- 認証は既存 `/status` と同じ。loopback以外は既存のBearer tokenが必要。
- 履歴はローカル `current_state/usage.sqlite` に30日・最大10万件。既存NAS配送ログは保持する。
- requestはresidentへの要求、attemptは上流への試行。両方を合算しない。cost guardの送信前拒否はattemptに含めない。
- peer resident経由では各ノードに記録されるため、全ノード合算は利用者の要求数と一致しない。
- 会話本文、秘密情報、生のエラー文は保存しない。tokens/cost未報告はnullとunknown件数で区別する。
- 料金のactual/estimateとキャッシュトークン（promptの内数）は別集計。
- Prometheusのcounterは起動ごとにリセットし、SQLite履歴の期限削除では減らない。期間内のincrease/rateはスクレイプ間の推定。
- 新しいモデル系列の初回サンプル以前の増分はPrometheusでは復元できない。正確な件数は起動後カウンタや個別履歴で確認する。
- request失敗時は最終応答モデルが未確定なのでtier/modelはunknown。モデル別失敗率はattemptで確認する。
- Grafanaの履歴は時系列とモデル別最終利用。個別呼び出し一覧は認証付きhistory APIで取得する。
- 過去NAS/spoolの自動取り込みはない。収集を有効にした時点以降が対象。

## 本番反映（承認後）

1. 対象Piの現行resident binaryとPrometheus設定を日時付きで退避する。
2. この変更を含む `kamimusuhi-resident` を対象アーキテクチャでビルドし、検証したbinaryだけを配置する。
   residentを再起動し、`/health` とloopbackの `/metrics` を確認する。
3. `prometheus.scrape.example.yml` の1 jobを既存scrape_configsに追加する。
   現在のPi設定は `/home/eightman/homelab/prometheus/prometheus.yml`。
   コンテナの `promtool check config /etc/prometheus/prometheus.yml` を通してからSIGHUPで再読込する。
   既存job、データソース、認証、保持期間は変更しない。
4. GrafanaにJSONをimportし、収集接続=1、履歴保存=1を確認する。
   通常利用後にモデル名・件数・履歴を確認する。確認のための有料呼び出しは不要。

不具合時は退避したbinaryと設定を復元し、resident再起動とPrometheus再読込を行う。
`usage.sqlite` と既存ログは削除しない。Grafana dashboard単体は既存Node Exporter画面に影響しない。

## 検証

```sh
cargo test -p kamimusuhi-resident
cargo clippy -p kamimusuhi-resident --all-targets -- -D warnings
python3 deploy/resident/monitoring/generate_dashboard.py
```

PromQLはGrafana変数を実値（例: `$__range`→`3h`, `$__rate_interval`→`1m`）に置換して
Prometheusの `promtool check rules` で検証する。本番へダッシュボードを追加しただけでは、
resident更新とscrape job追加が終わるまで未収集表示になる。
