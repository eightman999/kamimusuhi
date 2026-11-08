# Shared-experience grounding probe

固定synthetic会話と保存済みの生応答に対し、本番と同じ`DecisionEvidence`フィールド・型付きassessmentを使い、必要な場合だけ最大1回のPersona再生成を行う評価用コマンド。toolsを提供せず、canonical DBを開かない。

このコマンドは単一候補の判定・再生成を調べる。Runtime全体、invocation preparation、provider競争、fallback、最終送信、会話のDB書込みは実行しない。`delivery_eligible`は型付きモデル判定が受理したという意味で、本番での送信や事実の証明ではない。

## 実行

リポジトリrootから実行する。`--config`には既存のHTTP Persona設定、`--identity`にはその実行を許可するidentity contextをローカルファイルで渡す。設定が参照する認証環境変数は従来どおり利用する。設定・identity・資格情報を評価成果物やログへコピーしない。出力先は既存ファイル・symlinkを指定できず、新規0600で作成する。

```sh
cargo build --locked --manifest-path evaluation/continuity/grounding-probe/Cargo.toml
cargo run --locked --manifest-path evaluation/continuity/grounding-probe/Cargo.toml -- \
  --config /private/path/runtime.json \
  --identity /private/path/identity-context.json \
  --suite evaluation/continuity/shared-experience-20261109.json \
  --captures evaluation/continuity/shared-experience-results-20261109/raw/after-v3.json \
  --capture-run baseline \
  --limit 8 \
  --out /private/tmp/shared-experience-probe-new-result.json
```

`--capture-run baseline|candidate`は、指定したcapture内の1回目・2回目の反復を選ぶ。旧版・更新版はこのフラグではなく、ビルドしたソースとcaptureファイルで選ぶ。既定は`baseline`。`--limit`は1〜8、既定4。各選択caseと既知の失敗応答には最大1回の再生成を許す。既知応答の根拠あり対照と追加の純粋時間主張の2対照はassessmentのみ。

論理provider呼出しの上限は`limit * 3 + 6`で、8件なら最大30回。adapter内部の有限回のtransport/JSON repairは別なので、HTTP要求総数とは一致しない。timeoutは最大60秒に制限する。APIへの接続と再生成を行うため、既存の認証・identity pin・費用制御が有効な評価環境で実行する。

出力には元応答、再生成本文、各assessment、型付き判定、digest、試行数、失敗コードと使用ソースのdigestを保存する。失敗や保留を成功へ変換しない。実際に返されたモデル名・usageはこのinterfaceで取得できないため`null`。要求aliasを実際のモデル重み名として報告しない。

## 旧ソースとの比較

既定feature `scope-guidance`は最終版の`instruction_for_evidence`を使い、Grounding再生成から失敗応答本文を省き、同じ根拠のdigestと試行数を保持する。旧f31aae8のソースはこのAPIを持たないため、比較用checkoutの同じ`evaluation/continuity/grounding-probe/`へこの評価packageを置き、次のようにビルドする。

```sh
cargo build --locked \
  --manifest-path /path/to/before-checkout/evaluation/continuity/grounding-probe/Cargo.toml \
  --no-default-features
```

旧版は`as_instruction`と失敗応答本文を含む従来のrepair guidanceを使う。**現行ソースでfeatureを無効にしただけでは旧版比較にならない**。依存するassessment・Persona実装も旧checkoutのものを使う。packageは独立したworkspaceであり、相対依存先が各checkoutの`crates/`を参照する。異なる版のバイナリを同じtargetで上書きして混同しないよう、実際に実行したバイナリと出力を版ごとに保持する。

既存captureとsuite＋Persona seedから構成したcontextのdigestが一致しなければ実行を拒否する。比較では同じ固定8事例、sampling、model alias、tools無効条件を維持し、変更したsource digestを記録する。4つの補助対照は主8事例の集計に入れない。補助対照の一部は現在の質問が異なるため、groundingとtask_fitを別に読む。

[2026-11-09の全出力・評定](../shared-experience-results-20261109/README.md)に途中案・失敗・保留・評定者間の違いも保存した。人手評定や独立したモデル重みの評定ではなく、本番end-to-end検証は別途必要。
