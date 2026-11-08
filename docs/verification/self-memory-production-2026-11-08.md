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
この時点では本番会話書込みのend-to-end確認は未実施だった。続いてユーザーが明示承認し、以下の追加検証へ進んだ。

## 復旧

障害時は進行中作業を確認し、consumerを一時停止してbackupの2binだけをatomic復元する。
本番DBを古いsnapshotへ戻さない。manifestを消して自動acceptする運用も行わず、復元版の宣言をexact tokenで明示reviewする。
今回の配備・review・accept・復帰の詳細証跡は各nodeの上記backup内に保持する。

## push後のCI補修

K-CORE focused CIの`cargo test --lib`は、クリーンrunnerでprovider worker用の通常runtime実行ファイルを生成せず、selectionの3評価が`provider worker executable is missing`で失敗した。
ローカルの既存targetではこの前提が隠れていた。focused workflowと`ci-local.sh`へ、unit test前の
`cargo build -p kamimusuhi-runtime --bin kamimusuhi-runtime`を明示した。
空の専用targetでこの順序を再現し、selection評価 **5 PASS / 0 FAIL**。`bash -n`・差分検査もPASS。
アプリケーションのRust sourceや本番binaryは変更せず、上記配備コミットの同一性・指紋も維持する。

## 追加承認後の本番end-to-end検証

ユーザーの「本番個体への試験会話書込みも承認する」に基づき、Mac/Piそれぞれの本番
`POST /v1/kamimusuhi/talk`へ、個人属性・好みと区別した試験用subjectの会話を送った。
外部投稿やツール実行を依頼せず、試験識別語を返す会話だけを使用した。

### 初回に検出した実不具合と修正

両ノードでHTTP 200と応答を受け取ったが、DB監査で内部再試行前のturnに入力だけが残り、
`memory_writeback`の未完了markerが各1件増えたことを検出した。したがって初回はE2E成功としない。
既存residentはhostのDead/Ioを1回再試行するため、HTTP成功だけでは全attemptの永続化を保証しない。

原因は`serde_json`の既定parserによる浮動小数点往復だった。合成した`0.03f32`は
`0.029999999329447746`へserializeされた後、既定parserで`0.029999999329447743`に変わり、
`SqliteStore::append_in`の厳密readbackが`appended evidence readback mismatch`としてrollbackした。
本番の失敗turnにも同じ確率値があり、`persona.completed`後、`response_generated`保存前で停止していた。

修正commitは`a270a00d8cc03c926530d680c5005215118fa157`。
storeの`serde_json`に`float_roundtrip`を有効化し、比較を緩めず正確な数値往復を保証する。

- 新規回帰テストは修正前FAIL、修正後PASS。合成確率・telemetryのatomic保存、同一再append、再openを確認。
- 確率値を変更するSQLite triggerは引き続き拒否され、行とlineageがrollbackされることを確認。
- store全回帰 **82 PASS / 0 FAIL**、Clippy全target `-D warnings`・fmt・差分検査PASS。
- 限定sourceのAPI判断・継続性・CLI回帰 **24 PASS / 0 FAIL**。loopback fixtureのみを使用。
- ビルドsourceはCargo/crates/knowledge/deployの追跡済み286ファイルが修正commitとbyte一致。
  無関係な作業ツリー差分は含めていない。
- 完全source archive SHA256: `6c94d70fe7ad3b9be391140f397a2c87009cb216fbee7677f120df4198743a2d`。

### 修正後の確認方法と残る失敗証跡

新しい試験subjectで2往復する。1回目に識別語を提示し、2回目は識別語を再掲せず回答させる。
HTTP応答だけでなく、対応するDBの入力・生成・発話、`memory_writeback_verified`のreadback、
入力から発話へのlineage、2回目の発話から前turnの入力・発話へのlineage、resident履歴を照合する。
同一sessionであること、API発話前判断・応答評定、fallback/tool数、FI3、設定・資格情報metadata、
個体とcontinuity head、pending taskとprovider worker残留も確認する。

**初回の未完了markerは各1件を保全する。削除や成功イベントの補完は行わない。**
既存実装には失敗turnを監査付きでabortするAPIがないため、global statusは
`incomplete_or_in_flight` / unresolved=1を保持する。修正後の判定は新規turnごとの証跡と
「既知の未完了1件から増えないこと」に限定し、全履歴の未完了解消とは扱わない。
また、この識別語試験は人間による文体・関係性評定や長期の自己モデル成長の実証ではない。

### 修正後の実測結果

[本番E2Eの集計証跡](../../evaluation/continuity/production-e2e-20261108.json)に、初回失敗と修正後4turnの結果を分けて記録した。

| 確認 | Mac | Pi |
|---|---|---|
| 修正後の2応答 | PASS | PASS |
| 1回目 / 2回目の時間 | 27.760 / 20.482秒 | 16.999 / 8.322秒 |
| 識別語を再掲しない2回目の回答 | PASS | PASS |
| DB readback completion / 発話lineage | 各turn PASS | 各turn PASS |
| 前turnの入力・発話へのlineage / 同一session | PASS | PASS |
| resident履歴のanswered数 | 2件 | 2件 |
| decision fallback / tool / draft提出・採用 | すべて0 | すべて0 |
| 既知の未完了 / 新規未完了 | 1 / 0件 | 1 / 0件 |
| FI3 / 個体・head・設定・資格情報metadata | continue / 保持 | continue / 保持 |
| boot epoch / pending task / provider worker残留 | 56 / 0 / 0 | 72 / 0 / 0 |

修正版のnative releaseはMac 26.10秒、Pi 9分01秒。
配備backupは各node rootの`backups/self-memory-float-20261108/`。
2binだけを更新し、設定・DBを置き換えず、FI3のchanged componentsは両ノードとも空で追加acceptは不要だった。
Piのresident/Slack/AgentHub workerはactive、`NoNewPrivileges=yes`を保持した。
各nodeの`current_state/deployments/self-memory-continuity-2026-11-08.json`も修正版sourceと本番E2E証跡へ更新した。

| node | 修正版resident SHA256 | 修正版runtime SHA256 |
|---|---|---|
| Mac | `02a5f6628657bd50cbe40ab6742dcbe9814e3dc25595a181b1bac59fe1c00e94` | `c16d436e0f914b761b5693a3f814dffc7b4be44a37b64838d6e89d180308c70c` |
| Pi | `906e17871effdaeff7c57fca0b6c1c1dc1c55f036d9d2ade4d28888fee006bd2` | `cd75c7ba5f728003549f8a293b9c762979c2cbbd1db76e47976b45afb94ef3c3` |

修正source commit `a270a00`は、[K-CORE CI](https://github.com/eightman999/kamimusuhi/actions/runs/42490555343)・
[Rust CI](https://github.com/eightman999/kamimusuhi/actions/runs/42490555388)とも全段階PASS。
