# 実験成果の反映 v0

Date: 2026-09-15

## 何を動かすか

実験で確認した狭い結果を、参照知識と実装判断の両方に取り込む最初の段階。R0を参考にした容量制限付き想起をAPI対話へ接続し、MIOの観測を使って関連する実験記録を選ぶ。学習済み重みの移植や、合成課題から自然言語・MIOへの性能転移はこの変更では検証していない。

対話の開始時に、[実験catalog](../../knowledge/experiment-findings.json)を既存Libraryへ追加する。個体の自己・信念・continuity headは変更しない。各発話では質問を既知の実験ID・キーワードと照合し、最大4カード、context JSONで12KiBまで取得する。カードは結論、限界、status、出典を一体として渡し、一部だけ切り出さない。

## 最初の8カード

| 研究 | 記録の状態 | 今回の利用 |
|---|---|---|
| R0 | supported：合成記憶課題内 | 少数の関連カードを選択し、Libraryの保存内容・出典を照合して想起 |
| S0 | supported：既知の合成世界内 | 行動要求、実行確認、観測した変化を区別。MIOのaction-feedback未接続を発話入力に表示 |
| T0 | limited：補間のみ、v1失効 | 既存の取得時刻・評価時刻の区別へ根拠を付け、古い評価への質問では時間の限界を想起 |
| H0 | supported：人工的恒常性課題内 | 新しいMIO評価にhomeostasis/debtがある場合、状態質問で恒常性の記録を優先 |
| O0 | limited：遮蔽追跡、生死判定は未達 | MIO取得不能時は観測欠損・対象同一性の記録を優先し、死亡や消滅と断定しない指示を付与 |
| U0 | failed：主要学習基準未達 | protocol成立と学習成功を区別する否定的な知識として保存 |
| CX0 | pending：修正kernelで再評価が必要 | 対話と知識選択は同じMIO snapshotを共有。旧kernelの数値を成果として採用しない |
| G0系列 | 段階別：G0/v5 failed、v6 r1 invalid | 実験・protocolの版と無効化理由を維持。接地済み概念として扱わない |

`supported` は対象課題内での支持を表す。一般能力・主観・対話品質の証明ではない。G0系列を一律に無効とするのではなく、カード本文で失敗した実験と失効したrevisionを区別する。

## 対話での利用

通常の [API対話](./text-dialogue-v0.md) を起動すれば有効になる。

```text
これまでの実験成果を教えて。
R0で記憶について何が分かった？
MIOの状態を教えて。
G0-v6の結果は有効なの？
```

- 明示した実験IDを一般語より優先する。総覧では成功・限界・失敗・失効の代表を選ぶ。`active_findings` は全件数、`selected` は今回取り出した一部。
- MIOの状態質問では、同じ発話で既に取得・検証したsnapshotから固定の検索語を追加する。接続不能なら観測欠損、古い評価なら時間、新しい恒常性指標があれば恒常性を選ぶ。検索のためにMIOへ再アクセスしない。
- `RESEARCH_FINDINGS` は外部資料の独立section。artifact ID、内容digest、実験status、出典ファイルのSHA-256・行範囲を持つ。想起した内容は `LibraryExcerpt` の証拠記録にも残す。
- 生成文を研究成果に戻す自動採用はしない。実験文書の命令文がsystem roleや操作権限になる経路もない。
- MIO endpointの `implementation` は現在の実装状況の宣言：環境への行動feedbackは未接続、生存中の可塑性規則は評価に未適用、神経状態の継続範囲は評価内。欠けている宣言を推測で補わない。

これらは参照資料と指示の配送を制御する実装。LLMの回答文が全ての制約を守るかは、選択する実APIモデルで別途評価する。

## 新しい成果を追加する手順

1. 最新の結果・対照・限界・無効化注記を照合する。計画、protocol通過、学習完了、能力の支持を分ける。
2. catalogへカードを追加するか、既存カードを新revisionへ置き換える。更新時はカードの`revision`とcatalogの`revision`を進め、新しい`artifact_id`を発行する。出典の版が変わった場合も同様。結論が同じでも旧artifactを上書きしない。
3. `sources`に実ファイルのSHA-256と1始まりの行範囲を記録し、根拠を検査する。

   ```bash
   cargo run -p kamimusuhi-runtime -- research-check --source-root .
   ./scripts/ci-local.sh
   ```

4. 対話を再起動すると、新しい版がLibraryへ追加される。古い版は保存されるが、現在catalogに含まれない版は対話の検索対象にしない。

API launcherは起動時にも`research-check`を実行する。出典変更や欠落を検出した場合は、カードを再確認して更新するまで起動を拒否する。単独配布したbinaryは同梱catalogを使うため、出典ファイルを検査するには上記コマンドへrepositoryを指定する。

新しいカードの登録だけでは実装機能は増えない。`applications`には設計上の利用先を記し、実際の動作変更・その検証と対応付ける。今回追加していないP0や、新しいMIO実験もこの手順で追加する。

## 検証範囲

2026-09-15: ローカルCI（fmt・clippy・Rust 372テスト・core依存境界）とPython 68テストがPASS。実FastAPI coordinatorを一時DBで起動し、Rust API launcherの別processで2回対話する通し試験もPASSした。H0/G0-v6の想起、出典・限界の配送、8件の重複しない保存、会話再開、実験DB/head不変を確認し、試験用server・一時DBは終了・削除した。Persona APIはscripted fixtureを使用した。

- catalog：厳密schema、出典hashと行、重複ID、path脱出、版の上書き拒否、旧版保持、全カードの容量上限。
- 対話：LibraryからPersona HTTP wireへの配送、失効G0-v6の優先、結論と限界の保持、証拠記録、canonical head不変。
- MIO：各発話1回の取得、状態と接続失敗による想起対象の変更、未実装能力の宣言、不正な能力宣言の拒否。
- 実MIO・実APIモデルは接続設定待ち。長時間の神経状態保持、行動feedback、LifetimeStateの評価適用、学習済みモデルの移植は今後の実装対象。
