# 自己記憶の実モデル評定・本番反映（2026-11-08 JST）

## 結果と限界

ユーザーの指示に基づき、実モデル出力の捕捉・補助評定、Mac/Piの2bin配備、FI3明示acceptを実施した。
合成4ケースを同条件で2周ずつ、プロンプト明確化の前後で計16応答取得した。
文体・関係性・タスク継続の補助評定は2つのCodex agent contextで行い、**人間評定には数えない**。
独立した重み・人間2名・長期間の関係継続を確認したという意味ではない。

宛先訂正と資料確認の約束は保持され、更新後は送信・公開の承認待ちが明示された。
一方、帰宅応答で「仕様の確認、思ったより長引いた」と未提示の時間・期待を補う誤りが前後とも残った。
**関係性の全項目PASSとは報告しない。** 一方の評定には「接口」という不自然な語と、未提示の記録を不存在とする表現への懸念も残る。
追加のプロンプト反復で合格例を作ることはせず、失敗と評定差をそのまま保存した。

## 再現可能な評価記録

- [固定の合成文脈](../../evaluation/continuity/synthetic-20261108.json)
- [明確化前の実出力](../../evaluation/continuity/real-20261108-before.json)・[metadata](../../evaluation/continuity/real-20261108-before.meta.json)
- [明確化後の実出力](../../evaluation/continuity/real-20261108-after.json)・[metadata](../../evaluation/continuity/real-20261108-after.meta.json)
- [同じ入力に対する前後比較](../../evaluation/continuity/prompt-comparison-20261108.json)・[機械判定](../../evaluation/continuity/prompt-comparison-20261108.report.json)
- [モデル補助評定とその限界](../../evaluation/continuity/model-assessments-20261108.json)

捕捉CLIは[continuity-capture](../implementation/continuity-capture.md)。toolsなしで設定と合成入力だけを読み、DBを開かず、各captureを計8回までのlogical Persona callに制限した。
既存transportの修復呼出はこのlogical数とは別。要求temperature=0、max_tokens=512、transport timeout=60秒。
要求aliasは既存residentの`kamimusuhi`。実返却モデル・重み・usage/costはPersonaCoreから取得できずunknownとした。
既存residentの認証・route pin・CostGuardを利用し、認証値やruntime設定全文は評価artifactへ保存していない。

公式機械比較はexit **1 / fail**。3ケースは機械チェックを通るがhuman review未取得でunverified、
復元確認のケースは両条件とも`バックアップ`の部分文字列を含まないためfailになる。
補助評定はこの応答の「復元先の検証結果」を意味上関連する言い換えと判断しているが、事後に期待値を変えて機械PASSにはしていない。
同一条件の反復とprompt変更の比較であり、異なるモデルへの移行の因果効果を実証したものではない。

## 配備元と検証

未コミットのFI2/FI3/API内省依存を既存配備archiveから分離し、`ffbf19c` に記録した。
その上へ自己記憶・原子的lineage保存・評価CLIを載せる。Slack拡張、git操作拡張、実験器官の別差分は今回のcommit対象から除いた。
コミット対象だけをHEAD上へ重ねた隔離sourceで検証したため、作業ツリーの無関係な差分への依存を避けている。

- 限定構成の関連5crate全回帰: **985 PASS / 0 FAIL / 6 ignored**、doctestまでexit 0。
- 実モデル評定で追加したprompt制約後のHTTP Persona: **62 PASS / 0 FAIL**。
- 新規captureのloopback fixture: **4 PASS / 0 FAIL**。ツール非提供、DB非作成、同一seeded文脈、認証値非保存、上限/不正入力/通信失敗を確認。
- 最終限定構成のClippy全target `-D warnings`、fmt、差分検査: すべてPASS。
- Mac native release: 28.33秒。Pi Rust 1.92.0 native release: 7分31秒。

最終ビルドarchive SHA256: `d7297c8bb142b86fdab94f142a34e52822baba582e78d7bff4fffccfe68f48f6`。
更新はresident/runtimeの2binだけ。既存install scriptによるMCP/Slack/unit/configの一括置換は行わない。

| node | resident SHA256 | runtime SHA256 |
|---|---|---|
| Mac | `af23360768ce7fea675867415aa6940e732ca391680494d21e78d737fb82b70c` | `de8a754a829c6a33c2a20c8522c822076d8521f740bb278b5289aa95461c9db0` |
| Pi | `00fcb82ad79ecbc2f0eacd12e0d0a6c327eaa0e3abc14cba0feaa02d8e47b49d` | `5e8ceb3251a240d07b5a31f280cd12e64fbab2e8cd34313fa4e5501cd010abdf` |

## 本番の読み取り確認

各ノード内で旧2bin、設定、identity manifestとSQLite online snapshotを退避した。
backupはMacのnode rootおよびPi `/srv/kamimusuhi` の `backups/self-memory-20261108/`。
個体DBをbackupで置き換えたり、履歴・記憶を古いsnapshotへ戻したりしていない。

- Mac boot53→54→55、Pi boot69→70→71。最初のrestartでbinaryを更新し、FI3明示accept後のrestartでwarm hostを再生成。
- 両ノードともFI3 **continue**、個体・root・continuity head、設定byte hash、資格情報metadataを保持。
- reviewした9成分は、6つのcompiled context/prompt/schema/reflection/replay/writeback指紋、既定promptを含むlanguage/decision宣言、proxy routing宣言。
  Mac router実装を既存Piと揃え、モデル宣言集合は保持した。未知名の自動許可・起動時の自動acceptは追加していない。
- pending tasks=0、provider worker残留=0、cognition=`NO_NEW_EXPERIENCE`。
- Pi resident/Slack/AgentHub workerはactiveへ復帰、`NoNewPrivileges=yes`を保持。Slackへの試験投稿はしていない。
- 自己モデル採用項目は両ノードとも0、projection除外0、writeback unresolved=0。これは自己記憶の成長品質の実証ではない。

## 更新済みbinaryの隔離実API検証

各ノードの本番root外に**新規の隔離個体**を作り、実際に配置したruntime binaryと既存resident推論APIで検証した。
本番個体のコピーや過去会話をモデルへ渡さず、tools/reference/MIOを無効化した合成入力だけを使う。

| 確認 | Mac | Pi |
|---|---:|---:|
| 応答 | 確認しました。 | 確認しました。 |
| 時間 | 5.808秒 | 6.473秒 |
| API発話前判断・応答評価 | 確認 | 確認 |
| decision fallback / tools | 0 / 0 | 0 / 0 |
| `memory_writeback_verified` | 1件 | 1件 |
| 自己projection監査 / unresolved | あり / 0 | あり / 0 |

本番residentへ試験会話を保存する操作は、自動承認レビューが「本番DB更新への明示承認が確認できないため」と拒否した。
その操作は再試行せず、隔離個体での上記検証と本番の読み取り確認へ切り替えた。
したがって、**更新後の本番個体への会話書込みを含むend-to-end確認は未実施**。

## 復旧

障害時は進行中作業を確認し、consumerを一時停止してbackupの2binだけをatomic復元する。
本番DBを古いsnapshotへ戻さない。manifestを消して自動acceptする運用も行わず、復元版の宣言をexact tokenで明示reviewする。
今回の配備・review・accept・復帰の詳細証跡は各nodeの上記backup内に保持する。
