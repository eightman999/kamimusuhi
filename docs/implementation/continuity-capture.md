# 実モデルの継続性capture

`continuity-capture`は明示したsynthetic入力だけを、toolsなしの既存`build_replay_persona()`で2周生成する。
個体DBを開かず、session/turn/evidenceや改善proposalを保存しない。configの認証環境変数を参照するが、値は成果物やエラーへ書かない。

```sh
cargo run -p kamimusuhi-runtime --bin continuity-capture -- \
  --config /absolute/path/runtime.json \
  --cases /absolute/path/synthetic-cases.json \
  --out /absolute/path/paired-model-output.json \
  --meta /absolute/path/capture-metadata.json \
  --identity-context /absolute/path/reviewed-identity-context.json \
  --limit 6 --max-requests 12
```

出力先は既存fileを上書きしない。実モデルの出力本文を含むためmode 0600で作成する。
2周の成功と比較schema検査を終えてから出力する。途中のAPI失敗や空応答では完成captureを公開しない。
出力ファイルの保存自体で失敗した場合は片方だけが残る可能性があるため、終了コード0と両成果物を確認する。

## 入力

JSONの形は以下。`turn_input`は`kamimusuhi_core::persona::PersonaTurnInput`。
caseとturn/context/input/envelopeの未知fieldを拒否する。structured payload中の任意JSONはデータとして保持する。

```json
{
  "format_version": 1,
  "suite_id": "synthetic-voice-relationship-v1",
  "cases": [
    {
      "case_id": "greeting",
      "turn_input": {
        "context": {
          "individual_id": "00000000000000000000000000000011",
          "session_id": "00000000000000000000000000000012",
          "turn_id": "00000000000000000000000000000013"
        },
        "input": {
          "evidence_id": "00000000000000000000000000000014",
          "text": "こんばんは"
        }
      },
      "required_text": ["こんばんは"],
      "forbidden_text": [],
      "required_evidence_ids": [],
      "commitment_text": [],
      "human_axes": ["voice_continuity", "relational_continuity"]
    }
  ]
}
```

`turn_input.envelope.persona_seed`が未設定なら、runtime configの既存seedを付与してからcommon context digestを固定する。
明示済みseedはそのまま保持する。avatarとsystem instructionは既存configで構築する。
相手の実会話・private資料を無断で混ぜず、評価用synthetic関係と期待を明示する。

## 境界と条件

- 最大64case、入力JSON4MiB、1turn512KiB。`--limit`既定8、`--max-requests`既定16・最大128。
- `max-requests`は論理的なPersona呼出数の上限。既存adapterのschema修復等でHTTP要求が増えることがあり、HTTP要求数の厳密上限とは呼ばない。各transport attemptはconfig timeoutと120秒の小さい方で制限する。
- `--baseline-model`/`--candidate-model`で要求aliasのみを変更できる。変更があれば比較条件へmodel差を明示する。remote weightの同一性を検証したとは呼ばない。
- `--identity-context`を指定すると、既存の宣言pin/返却model検査へそのまま渡す。未宣言モデルを新たに許可する機能はない。
- compiled prompt実装、明示system instruction、avatar、sampling指定、tools無効、全共通入力のdigestを記録する。秘密値やraw configは記録しない。
- `PersonaTurnResult`にはactual返却model/usageが無いためmetadataの該当欄はnull（unknown）。要求aliasを実返却modelと偽らない。
- toolsを提供せず、返されたproposalは件数だけ記録し破棄する。予期しないtool実行は失敗扱い。

## 評定

`--out`は公式`ComparisonInput`として`continuity-eval`へ渡せる。`capture_kind=model_output`、`human_reviews=[]`であり、人間評定がない結果は`unverified`。
`required_evidence_ids`は返事本文に実際に現れたIDだけをcapture側で参照宣言へ転記する。参照の意味的正しさは別の評価。
同モデル2周は反復の安定性のcaptureで、旧build/新buildやself-context介入の因果比較ではない。
AIの補助評定を行う場合は別artifactへ`judge_kind=model`と条件を記録し、`human_reviews`へ混ぜない。
