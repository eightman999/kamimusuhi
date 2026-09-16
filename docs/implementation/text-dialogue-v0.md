# Text dialogue v0 — APIで状態と経験を言葉にする

Date: 2026-09-15

## 到達点

既存の Rust runtime に自由入力の `chat` / `talk` を追加する。同じ個体を復元し、相手ごとの会話原記録、保存済みの記憶、実測の稼働状態を Persona Core へ渡し、短い日本語の応答を得る。

言語生成には操作者が指定するAPIを使う。対応するのは Bearer 認証付きの OpenAI-compatible Chat Completions 形式（base URL に `/chat/completions` を追加）。Anthropic等の独自wire形式へ直接接続するadapterは含まない。MIOの完了済み評価記録を読み取り、発話の入力へ渡せる。実行中の神経状態のストリーム、自律的な発話開始、音声出力は未接続。

## 開始

```bash
export KAMIMUSUHI_API_BASE_URL='https://your-provider.example/v1'
export KAMIMUSUHI_API_MODEL='your-model-name'
# KAMIMUSUHI_API_KEY にAPIキーを環境側で設定してから:
./scripts/chat-api.sh
```

URL・モデル名は実際の提供元の値へ置き換える。キーの値をCLI引数や設定ファイルへ書かず、`KAMIMUSUHI_API_KEY` 環境変数から呼び出し時に読む。既存の環境変数を使う場合は `KAMIMUSUHI_API_KEY_ENV` にその**変数名**を設定する。

初回は `.local/text-dialogue` に新しい個体を作り、次回から同じ個体と原記録を読む。既存の runtime が不完全なら自動初期化せず失敗する。APIへは現在の入力、直近の会話、相手に対応する保存済み記憶、稼働状態を送る。

最初に試す入力:

```text
こんにちは。今どんな状態で動いているの？
私は朝の散歩が好きです。
さっき私が好きだと言ったのは何？
/quit
```

保存先、会話相手を変える場合:

```bash
./scripts/chat-api.sh .local/text-dialogue local-user
```

`subject` はローカルの操作者が選ぶ相手のキーで、認証機能ではない。同じキーの最近の会話を再開時に読み込む。相手を変えるときは異なるキーを使う。

直接 CLI から接続する場合:

```bash
cargo run -p kamimusuhi-runtime -- init --dir .local/my-dialogue
cargo run -p kamimusuhi-runtime -- chat --dir .local/my-dialogue \
  --persona openai-compatible --persona-url "$KAMIMUSUHI_API_BASE_URL" \
  --persona-model "$KAMIMUSUHI_API_MODEL" --persona-auth-env KAMIMUSUHI_API_KEY \
  --persona-locality external --privacy unconstrained
```

`talk --message '今どんな状態？'` は1回分の応答を JSON で出力する。`chat` は1行ごとに応答し、`/quit` または EOF で終了する。直接 CLI の既定は `local-only` なので、外部APIを使う際は毎回 `--privacy unconstrained` を指定する。API launcher は指定済み接続先への送信を明示して起動する。テスト用の固定応答は `--persona fake` と明示した場合だけ選べる。

## MIOの接続設定

接続先・個体は後から指定できる。MIO coordinator URL、実験ID、genome IDの3つを固定して指定する。coordinatorには本変更の `GET /api/dialogue/organisms/{genome_id}` が必要。

```bash
export KAMIMUSUHI_MIO_URL='http://your-coordinator:8870'
export KAMIMUSUHI_MIO_EXPERIMENT='your-experiment-id'
export KAMIMUSUHI_MIO_GENOME='your-genome-id'
./scripts/chat-api.sh
```

`your-*` は実際の値へ置き換える。候補個体の自動選択は行わず、応答の実験ID・genome IDを指定値と照合する。直接CLIでは `chat` / `talk` に `--mio-url URL --mio-experiment ID --mio-genome ID` を付ける。指定は `runtime.json` の `mio` に保存され、次回以降も使う。環境変数を未設定にするだけでは保存済み接続を解除しない。解除は次の対話起動で `--no-mio` を指定する。

- 各発話の前にGETを1回行う。APIキーや会話本文はMIOへ送らず、取得した観測を発話APIへ渡す。
- 取得するのは個体の構造と、同じ実験・個体に属する成功jobの最新評価。活動率、spike数、恒常性・回復の指標、評価ID、backend、科学設定hash、評価完了時刻を含む。`active_fraction` は活動した評価replicateの割合で、活動した神経細胞の割合ではない。
- 取得時刻と評価完了時刻を区別する。既定の鮮度基準は300秒。古い評価には `stale_record`、完了時刻不明には `unknown_timestamp`、有効な評価記録なしには `no_record` を付ける。欠測から未評価とは断定しない。新しい取得時刻だけでは、古い評価を現在の状態と扱わない。
- `KAMIMUSUHI_MIO_MAX_AGE_SECS` / `--mio-max-age-secs` で鮮度基準を1〜86,400秒に変更できる。応答の取得時刻自体が基準より古い場合も拒否する。時計の未来方向のずれは5秒まで。HTTP timeoutは既定2秒、`runtime.json` の `mio.timeout_ms` で1〜30,000msに設定できる。
- 不明・不正な測定値はcoordinator側で省略する。Rust側でもschema、型、範囲、個体、時刻を検証する。無効応答・接続失敗時は `connection=unavailable` として会話を続け、前回の観測で埋めない。観測JSONは32KiBまで。
- `MIO_OBSERVATION` は観測のevidence IDと検証後snapshotのdigestを持つ独立section。`backend=mock` は模擬評価と明示する。実験のgenome IDをcanonical個体IDと同一視せず、記録から感情・空腹・痛みを推定しない。
- 接続は読み取り専用。実験の開始・停止、worker操作、genome変更、fitness更新は行わない。

## データと挙動

[実験成果の反映 v0](./research-integration-v0.md) により、レビュー済みの研究カードをLibraryへ蓄積し、質問とMIOの状態に関連するカードを最大4件取得する。`RESEARCH_FINDINGS` は結論・限界・status・出典付きの独立sectionで、個体の経験や獲得済み能力とは区別する。API launcherは起動時に出典ファイルのhashも確認する。

- 1入力は UTF-8 8,192 bytes まで。直近12メッセージ、原文合計16,384 bytes までを古い順に渡す。上限超過時は古い原記録を入力から外し、DB 原文は保持する。
- 会話履歴は同じ individual / subject の user・assistant 原記録だけ。保存済み記憶も同じ subject の relationship / episodic だけを取得する。
- `CONVERSATION_HISTORY` と `OBSERVED_RUNTIME` は独立した JSON section。実測状態には OS、CPU architecture、session の経過秒数、turn 数、今回利用できる履歴数などを含める。感情・空腹・神経活動をこれらから推定しない。
- 原記録の出典をJSONに保持したうえで、履歴をAPIのuser/assistantメッセージにも渡す。現在の相手の入力を最後に置く。履歴の文章からsystem/tool roleは作れない。
- モデルの生成結果は `SystemEvent(response_generated)` として記録し、出力先への write/flush 成功後に `AgentUtterance` を追加する。出力失敗した生成文は次の会話履歴に含めない。出力先の受領は、人が読んだ・同意したという証明ではない。
- 生の会話は SQLite evidence store に保存する。operational trace は ID、digest、件数だけを記録する。
- `runtime.json` に保存する認証情報は環境変数名だけ。接続先URLを変えると旧接続先の認証設定を引き継がない。新しい送信先には `--persona-auth-env` で改めて変数名を指定する。
- 発話は自己・関係・信念の mutation を起こさず、continuity head を進めない。モデルが返した draft もこの経路では採用しない。

出力成功直後、保存前にプロセスが終了した場合は、表示済みの文が履歴に記録されない可能性がある。端末出力と SQLite を一つの transaction にはできず、この段階では再送・完全な配送確認は実装していない。

## 検証

- core / HTTP adapter: 旧 envelope の deserialize 互換、履歴の role/evidence ID、独立 section と改行の保持。
- runtime integration: HTTP wire、複数 turn と別processでの再開、subject 分離、head 不変、privacy 事前拒否、出力失敗、入力・履歴上限、trace への原文非掲載。
- API認証: Bearer header、環境変数名だけの設定保存、接続先変更時のcredential非継承をscripted endpointで確認する。
- MIO: Python coordinatorの読み取り専用性・記録の所有関係と欠測処理、Rustの個体照合・鮮度判定・異常応答拒否・接続解除、MIOからPersona/evidenceまでの経路をfixtureで検証する。
- 実API接続と選択したモデルの言語品質は、接続先・モデルが確定してから検証する。構造テストの PASS を言語能力の PASS と呼ばない。

### 2026-09-15の検証結果

- `./scripts/ci-local.sh`: 研究成果の反映を含め、fmt、clippy、workspaceの372テスト、core依存境界がすべてPASS。
- `.venv/bin/python -m pytest experiments/mioba/tests/test_dialogue_snapshot.py experiments/mioba/tests/test_coordinator.py experiments/mioba/tests/test_m1_observatory.py experiments/mioba/tests/test_gui.py -q`: 68 PASS。既存依存ライブラリ由来のwarning 4件。
- 一時DBを使う実FastAPI coordinator → Rust `scripts/chat-api.sh` の2往復 → scripted Persona APIで、観測・出典・履歴・設定保存、MIOへのキー/本文非送信、実験DBとcanonical headの不変を確認。追加のwire試験でu64最大spike数の原値保持と `no_record` も確認。試験用serverと一時DBは終了・削除済み。
- 実際の接続先・対象個体は後から設定する。実MIOサービス、実APIモデル、実行中の神経状態・音声出力はこの検証に含まない。

### 開発中の予備観察

`qwen2.5:0.5b` では3つの別 CLI process による応答生成と、履歴件数 `0 -> 2 -> 4`、同じ individual の復元を確認した。一方、稼働状態の質問に答えず、相手の「朝の散歩が好き」を自分の好みとして話した。初回の意味的な品質評価は **FAIL**。この結果を対話品質の達成根拠に使わない。

ローカル1.5Bモデルの予備試験でも、JSONだけでの会話表現では自己/他者の混同が残り、APIの会話roleを併用すると過去発言の参照は改善したものの、未接続のセンサーから情報を得たと述べる誤応答があった。これらはAPIモデルの評価結果ではない。ローカル推論用launcherは成果物に含めない。

## 次の接続

1. 保持した会話・状態に対する、日本語応答の正確性と自己/他者区別を評価する。
2. MIOの実行中の神経状態を渡す場合は、worker側の観測契約と取得経路を追加する。
3. 内部状態や変化を根拠とした発話意図を生成し、発話しない選択も含めて評価する。
