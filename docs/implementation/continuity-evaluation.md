# 継続性の対比較評価

`continuity-eval` は記録済みのbaseline/candidate出力を同一ケースで比較する。
API・本番個体DBを開かず、課金も行わない。実モデルの出力を収集済みなら同じ形式で入力できる。
fixtureは評価器の動作だけを示し、実モデルの人格や継続性の性能ではない。

```sh
cargo run -p kamimusuhi-runtime --bin continuity-eval --offline -- \
  --input /absolute/path/paired.json --out /absolute/path/report.json
```

終了コードは比較全体の`comparison_status`に従い、0=PASS、1=検査FAIL、2=入力拒否、3=人間評価未完了（UNVERIFIED）。
`candidate_status`がPASSでもbaselineの評定未完了なら比較全体はUNVERIFIED。
レポートに本文は含めず、出力・datasetのdigest、条件、case別の結果と制約を残す。
入力と同じパスへの出力は拒否する。

## 入力契約

`continuity_eval::ComparisonInput` が正典schema。未知fieldは拒否する。

- `format_version`: 1、`suite_id`: 評価集合ID。
- `cases`: 一意の`case_id`、同一の`input`、実際に渡した文脈の`context_digest`。
- caseの`required_text`/`forbidden_text`は完全な部分文字列の期待/禁則。
  `required_evidence_ids`は記録された参照IDの包含、`commitment_text`は約束の言及を確認する。
  `human_axes`は`voice_continuity`、`relational_continuity`、`task_continuity`から指定する。
- `baseline`/`candidate`: 異なる`run_id`、`capture_kind`（fixture/model_output）、`conditions`、`condition_digest`、`outputs`。
  各outputはcaseと同じID・入力・context digest、空でない`text`、`cited_evidence_ids`、`human_reviews`。
- `conditions`: `model`と、`prompt_digest`/`sampling_digest`/`tools_digest`/`memory_digest`。
  digestは`kamimusuhi_core::digest::content_digest`等で捕捉時に固定する。
  `condition_digest`は`RunConditions::digest()`の値。digest形式は`sha256:`＋64桁hex。
- `allowed_condition_changes`: 比較で変更を許す成分の明示リスト。
  モデル比較なら`["model"]`のみ。宣言なしのprompt/記憶/tool差は拒否する。
  caseのcontext digestは常に同じでなければならない。

fixtureと実モデルcaptureの混在、欠落・余剰・重複case、空出力、入力/context違い、digest不一致、許可外の条件変更は
採点前に全体を拒否する。空のcase契約や矛盾した期待/禁則も成功にしない。
レポートの`changed_conditions`に比較条件の変化を記録する。

人間評定は各outputの`human_reviews`に`reviewer_id`、`axis`、`judgment`（pass/fail）、
`rationale`を書く。要求axisごとに異なる評定者2人以上のPASSが必要で、1人でもFAILならFAIL。
未評定や1人だけ、human_axes自体が未指定の場合もUNVERIFIED。評定者の実在・独立性はoperatorが確認する。
文字列のIDを変えるだけで独立評定が成立するという意味ではない。

## 何を測るか

5本の継続性評価のうち、モデル変更前後の振る舞い比較をこのharnessで扱う。
同じ訂正・約束・教訓を文脈に入れた同じ問いを両条件へ渡し、その記録を比較する。
他4本（訂正逆戻り、再起動後の約束、自己更新の欠落、書戻停止）はruntime/storeの隔離
統合評価で状態遷移自体を検証し、生成済み応答があればこのharnessへも入力できる。

機械検査は文字列や宣言参照の照合であり、引用元が主張を含意するかは判定しない。
否定文にも期待語が出るため部分文字列PASSだけで正答とは扱わない。
約束に言及したことと現実に履行したことも別。
口調・関係の温度を単語一致や出力digestだけで保証しない。
同じ設定名でも外部モデルの重みが同じかは確認できず、条件・文脈の捕捉が真実であるかも
このoffline評価器自体では証明できない。

`cargo test -p kamimusuhi-runtime --test continuity_eval --offline` が、訂正/約束の退行、
人間評価未完了、関係性FAIL、条件汚染、欠損・重複出力、CLI終了コードを検証する。

## 実行できる入力例

`crates/kamimusuhi-runtime/tests/fixtures/continuity-paired.json`は人工の入力例で、
実モデルの測定値ではない。同じコマンドの`--input`へ渡すと、機械検査は通り、
人間評定が未実施なので終了コード3と`candidate_status: unverified`を返す。
実測時は`capture_kind: model_output`として、両runの条件・実際の入力文脈・応答を捕捉する。
fixtureと実モデル出力を混ぜた比較は拒否する。評定欄を埋めるのは実際の独立評定後に限る。

## 検証結果（2026-11-08）

- `cargo test -p kamimusuhi-runtime --test continuity_eval --offline`: **13 PASS / 0 FAIL / 0 ignored**。
- `cargo clippy -p kamimusuhi-runtime --bin continuity-eval --test continuity_eval --offline -- -D warnings`: PASS。
- 同梱fixtureを実CLIのstdoutと`--out`の両経路で実行: exit 3、機械4検査true、
  voice/relationalは各0評定・UNVERIFIED。JSONとして再読込でき、応答本文はレポートに含まれない。
- 実LLM要求・追加課金・本番個体への書込みは行っていない。
