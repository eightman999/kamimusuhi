# 未完了の書戻しを監査付きで終端にする

`memory_writeback_verified`は証跡の保存成功を表す。失敗したattemptをこのイベントで補完しない。
運用者が明示的に放棄する場合は、markerに予約された同じcompletion IDへ
`memory_writeback_aborted`をappendし、成功と区別する。元marker・入力・生成証跡は保持する。

## 操作

```sh
kamimusuhi-runtime writeback-review --dir "$individual_dir" \
  --turn-id "$failed_turn" --reason "$operator_reason"

kamimusuhi-runtime writeback-abort --dir "$individual_dir" \
  --turn-id "$failed_turn" --reason "$operator_reason" \
  --review-token "$reviewed_token"

kamimusuhi-runtime memory --dir "$individual_dir"
```

`writeback-review`はDBを書き換えず、markerの生byte digest、元EvidenceRecord全体のdigestとID、
対象個体/session/turn/completion、理由に結び付いたreview tokenを返す。
`writeback-abort`は同じ状態を再読込し、一致した場合だけ失敗終端を保存する。
同じ理由・tokenによる再実行は同じイベントを返し、行を増やさない。
理由はtrim後1〜2048 byte。必須引数の欠落、IDの形式不正、他用途のCLIオプション混在はRuntimeを開く前に拒否する。

これはローカル運用者用のCLIで、OSのファイル・プロセス権限に従う。
review tokenは対象を固定する値であり、認証情報ではない。
本番での実行には対象のDB更新の承認が必要で、自動リフレクションやモデルには実行させない。

## 進行中turnとの排他

通常turnは開始から発話後の永続書戻しまで`identity-use.lock`の共有leaseを持つ。
review/abortは同じleaseを非blockingで排他取得する。個体のいずれかのturnや
reflectionが稼働中なら`MEMORY_WRITEBACK_BUSY`として拒否する。
abortはその下でwriter lockを取得し、既存のidentity acceptanceと同じ取得順序を守る。
FI3の承認済み宣言とも照合する。稼働中の処理をtimeoutで失敗と推定することはしない。

## 記録と状態表示

失敗終端はSystemEvent / Observedとして次を保存する。

- `event=memory_writeback_aborted`
- `resolution=operator_abandoned_incomplete_writeback`
- `actor=operator`と運用者が指定した`reason`
- `marker_sha256`、`evidence_snapshot_digest`、`evidence_ids`
- 元証跡それぞれへの`derived_from` lineage

イベントとlineageは同一transactionで保存・読戻しする。既存reserved completion IDを上書きしない。
保存済みverified、未知の終端、誤った個体/sessionのmarker、壊れた監査、既存発話行のあるturnは拒否する。
発話行がないことは、人間に一度も表示されなかったという証明ではない。
この操作は未完了attemptの放棄を記録し、配達や意味上の記憶品質を認定しない。
UserUtteranceを含まないorphan markerや、発話後にcompletionだけ失敗したturnの自動回復は対象外。

`memory`の`memory_writeback`は以下を分けて表示する。

| 項目 | 意味 |
|---|---|
| `unresolved` / `turn_ids` | 終端を確認できないattempt |
| `aborted` / `aborted_turn_ids` | marker・元証跡・lineageを照合できた失敗終端 |
| `state=no_unresolved_turns` | 未完了が0件。過去の失敗が0件という意味ではない |

statusの読取りは元証跡のsnapshotと正確なlineage集合を再検証する。壊れた監査を解消済み扱いしない。
markerは最大4KiB、1turnの元証跡は最大4095件。全体の未完了は128件、保持するabort markerは4096件までを検査する。

## 検証

`tests/writeback_abort.rs`は元証跡保持、再起動、冪等性、共有lease/実際のemit中の拒否、
証跡変更によるtoken失効、既存発話・verifiedの拒否、lineage書込失敗のrollback、
監査/marker改竄の検出を確認する。
`tests/writeback_abort_cli.rs`は実CLIでreview→abort→retry→memoryと不正引数の無書込みを確認する。
