# 書き込み経路の「壊れ方の形」3種と障害注入テスト（2026-11-27 JST）

別システム FIO（Python 製の常駐エージェント）の記憶書き込み経路に見つかった3つの壊れ方の形を題材に、
かみむすびの書き込み経路が同じ形で壊れないかを読み、足りない所を障害注入テストで固定した。
FI1（ターン・トランザクション）の disposition「既存 Rust kernel との差分を整理し未実証の障害だけ評価」
（[experiment-dispositions](../research/experiment-dispositions-2026-11-08.md)）に沿う作業で、Python 実験側の結果ではない。

**結論: 3形のうち、Continuity Kernel・証拠追記・対話ターンの書き戻しは同じ形で壊れない。
一方で (a) の形が intercom の受信処理に、(b) の形が C0 派生レーンの canonical narration に残っており、
どちらも `#[ignore]` 付きの再現テストで再現した（本体コードは未修正）。**

## 3つの形（FIO 側の事実）

| 形 | FIO で見つけた書き込み順序 | 壊れ方 |
|---|---|---|
| (a) 冪等の鍵を処理の前に立てる | 書き戻しの入口で内容 hash（時間窓つき）の「処理済み」鍵を記録し、そのあと各段を実行する | 途中で失敗しても鍵が残り、同じターンの再送が重複として捨てられる。失われた書き込みは二度と入らない |
| (b) 投影を先に書き、台帳の失敗を読み戻さない | 派生テーブルを先に更新し、後から追記専用の台帳に記録する。台帳追記の失敗は別ファイルへ退避する | 退避ファイルを読み戻すコードが無く、投影と台帳が食い違ったまま気づかれない |
| (c) 派生物へのミラーが主保存の成否を見ない | 主ストアへの保存の成否（失敗・重複・拒否）に関係なく派生物へ書く | 主には無いのに派生にだけ在る記録ができ、逆向きの不整合も起きる |

## 対応表

「守る仕組み」は実コードを読んで確認した事実。「テスト」の **太字** が今回追加したもの。

### (a) 冪等の鍵を処理の前に立てる

| かみむすびの該当箇所 | 守る仕組み | 覆っているテスト |
|---|---|---|
| Continuity Kernel の `idempotency_key`（`kamimusuhi-store-sqlite/src/continuity.rs` `activate`） | 鍵は `activation_receipts.idempotency_key` にだけ在り、proposal・state・commit・head・receipt・audit と同じ IMMEDIATE トランザクションで書かれる。照合は receipt と `payload_fingerprint` に対して行い、失敗した試行は鍵を残さない。鍵は `<scope>/<session>/<turn>/draft-<n>` で、内容 hash でも時間窓でもない（`runtime/src/c0.rs` `submit_drafts_scoped`） | `transaction_failpoints::failures_before_commit_roll_back_every_activation_row_and_recover_on_retry`（同じ proposal ID の再試行）、`continuity_activation::same_idempotency_key_with_same_content_under_new_id_is_deduplicated`（成功後の重複）、**`store-sqlite/tests/write_fault_shapes::a_failed_activation_does_not_reserve_its_idempotency_key_for_a_resend`**（FP01–FP06 の各失敗後に、新しい proposal ID・同じ鍵で再送すると `Activated` になり、receipt・state が1件ずつ在る）、**`…::a_failed_activation_does_not_turn_a_corrected_resend_into_a_payload_mismatch`**（失敗後に同じ鍵・別内容で再送しても `DUPLICATE_PROPOSAL_PAYLOAD_MISMATCH` にならない） |
| 対話ターンの writeback marker（`memory-writeback/<turn>.json`、`runtime/src/memory_writeback.rs`） | marker が表すのは「処理済み」ではなく「未完了」。完了 `memory_writeback_verified` は入力・生成・発話の読戻し照合後、最後に予約 completion ID へ追記する。marker の鍵はターンごとに新しく生成される `turn_id` で、内容では照合しない。失敗ターンは `unresolved` として残り、後の成功で消えない | `continuity_scenarios::continuity_04_delivered_response_with_failed_evidence_write_is_detected_after_restart`、`continuity_scenarios::failed_lineage_write_cannot_leave_an_unlinked_emitted_answer_in_history`、`writeback_abort.rs` 一式、**`runtime/tests/write_fault_shapes::a_resent_text_after_a_failed_writeback_is_recorded_not_deduplicated`**（同じ文面の再送が別ターンとして UserUtterance に2件残り、再送側の verified が再送の入力を指し、失敗ターンは unresolved のまま） |
| 証拠の追記（同じ `evidence_id` の再追記、`append_with_links`） | 鍵は証拠行そのもの。行と lineage を1トランザクションで書き、同一内容の再追記は既存行を返す | `evidence_atomic::failure_on_second_link_rolls_back_emitted_evidence_and_the_first_link`（失敗後に同じ ID で再追記できる）、`evidence_atomic::atomic_append_survives_restart_and_identical_retry_without_duplicates` |
| node 間 task の冪等キー（`resident/src/delegation.rs`） | 鍵は `task_id`＝実行前に SQLite へ永続化した task 行。再起動時に `delegating`/`running_remote`/`running` を `retry_pending` に戻して再実行する | `delegation::tests::store_round_trip_and_recovery`、`resident/tests/node_delegation::restart_restores_pending_tasks_and_recovery_completes_them` |
| **intercom の受信（`resident/src/intercom.rs`）— 形 (a) が残っている** | `accept` は `seen` への id 追加と `inbound` への追加を同じ `intercom.json` 書き込みで行う（ここは健全）。しかし `drain_inbound` は envelope を `inbound` から pop して **保存してから** 対話ターン（`dialogue::talk`）を実行する。ターン中にプロセスが落ちると、ディスク上は「id は seen、envelope は inbound に無い」状態になり、送信側の再送は `dedup` で捨てられる | **`resident/tests/intercom_inbound_crash::an_envelope_in_flight_at_a_crash_is_not_lost_or_deduplicated_on_resend`（`#[ignore]`、欠陥の再現）** |

### (b) 投影を先に書き、台帳の失敗を読み戻さない

| かみむすびの該当箇所 | 守る仕組み | 覆っているテスト |
|---|---|---|
| canonical commit と `state_records`（投影）/ `mutation_decisions` / `audit_events` / `activation_receipts`（台帳） | 書き込み順は FIO と同じく投影（`state_records`）が先だが、全行が1つの IMMEDIATE トランザクション内。`state_records.created_commit_id` は DEFERRED FK で後続の commit 行を要求する。失敗は全行ロールバック、COMMIT 後の失敗（FP07）は receipt から復元 | `transaction_failpoints.rs` 3本（FP01–FP07、子プロセス abort を含む）、**`store-sqlite/tests/write_fault_shapes::projection_and_ledgers_agree_after_failures_at_every_window`**（全 failpoint で1回ずつ失敗→再試行した後、投影⊆台帳・台帳⊆投影、commit ごとに state/receipt/accept 決定/`continuity.activated` 監査が各1件、失敗試行の proposal 行が残らないことを SQL で照合） |
| C0 派生レーン内部（`c0_*`: proposal / activation / snapshot / head） | snapshot・status・head を1トランザクションで更新 | `c0_integrity::write_landing_fault_rolls_back_snapshot_status_and_head_together`、`c0_integrity::a_head_write_that_does_not_land_is_not_acknowledged` |
| **C0 レーンの canonical narration（`runtime/src/c0.rs` `activate` / `rollback` / `propose_param`、`c0/reflection.rs`・`c0/replay.rs` の `narrate`）— 形 (b) が残っている** | モジュール文書は「すべての lifecycle 遷移を canonical evidence にも書く」とする。しかし `activate` は `c0_apply_activation` をコミットしてから、別の書き込みで `c0_activation_applied` を追記する。追記が失敗するとレーンは動いたまま呼び出し側にはエラーが返り、再試行は `ProposalNotPending` で拒否されるため narration は二度と書かれない。narration を読み戻して照合するコードは見当たらない（`c0_activation_applied` の読み手は無い） | **`runtime/tests/write_fault_shapes::c0_lane_and_its_canonical_narration_agree_after_a_failed_narration`（`#[ignore]`、欠陥の再現）** |
| spool（fsync 付き追記 → NAS、`resident/src/spool.rs`） | ローカル spool が先で NAS が後。NAS への配送に失敗した `.ready` バッチは spool に残り、次の `deliver` が走査して再送する（退避先を読み戻す）。配送は at-least-once で、各レコードは一意の `id` を持つ | `spool::tests::failed_delivery_keeps_spool`、`spool::tests::journals_append_and_deliver_at_least_once` |
| `state_record_usage`（想起の使用記録） | 台帳ではない副テーブル（`mark_used` は失敗を無視する best-effort）。canonical の内容には影響しない | 対象外（下の「観察」参照） |

### (c) 派生物へのミラーが主保存の成否を見ない

| かみむすびの該当箇所 | 守る仕組み | 覆っているテスト |
|---|---|---|
| 対話ターンの派生記録（`memory_writeback_verified`、ターン評価 `c0_evaluations`、trace の `FinalExpression`/`ResponseEmitted`） | いずれも発話 AgentUtterance の追記に `?` で続く。主保存が失敗すると書かれない（trace は静的確認のみ） | **`runtime/tests/write_fault_shapes::a_failed_primary_write_leaves_no_derived_completion_or_turn_evaluation`**（失敗ターンに verified・`c0_evaluations` が0件、成功ターンでは各1件の陽性対照つき）、`continuity_04…` |
| 配達と永続化の区別（配達検証前の発話、`defer_delivery`） | 画面への配達は永続化より先に起こりうるが、それを成功とは扱わず marker が unresolved を示す。`defer_delivery` は保留提案として保存し、後の配達検証で確定する | `continuity_04…`（配達済み・永続化失敗が再起動後も検出される）、`writeback_abort::emitted_but_unverified_turn_cannot_be_abandoned_as_an_unemitted_attempt` |
| resident の対話ジャーナル（`conversations/dialogue`） | 受信時に `received`、結果は host の成否で `answered` か `failed` を書き分ける。失敗ターンが `answered` になることはない | `resident dialogue::tests::a_failed_turn_still_leaves_the_received_message`、`…::a_successful_turn_journals_intake_then_answer` |
| Continuity Kernel の拒否（重複・stale・domain separation） | 拒否は proposal・決定・監査だけを記録し、`state_records` を書かない | `continuity_activation::rejected_proposal_is_durable_and_does_not_move_head`、`stale_writer.rs` |
| `self_notes.jsonl` | 該当なし。canonical store の写しではなく単独の作業メモ（`runtime/src/self_notes.rs` の文書どおり）。ミラー元が無い | — |
| Library | 該当なし。artifact と chunk を1トランザクションで書き、別形式の写しを作らない（`store-sqlite/src/library.rs` `import`） | `library_store.rs`、`restart_vertical_slice::the_library_survives_the_restart_and_is_not_reimported` |
| tool 呼び出しの `ResourceResult` 証拠 | 該当なし。外部の結果を一次記録として追記するもので、主ストアの写しではない | — |

## 見つけた欠陥（本体コードは未修正）

### 欠陥1: intercom の受信 envelope がターン中のクラッシュで失われ、再送も捨てられる（形 a）

- 再現: `env -u HAI_API_KEY cargo test -p kamimusuhi-resident --test intercom_inbound_crash -- --ignored`。
  台本の runtime がターン内に入ったことを marker で確かめ、その時点の `intercom.json` をクラッシュ時のディスク像として別の resident に読ませる。
  結果は `inbound=[]`、`seen=["m-crash-1"]`、再送は `{"accepted":false,"dedup":true}`。
- 影響: 送信側に `accepted:true` を返した手紙が処理されず、再送も受け付けない。
  intercom の journal に `received` は残るが、それを読んで再投入するコードは無い。
  [intercom 実装文書](../implementation/intercom.md) の「受理済みの手紙は再起動を生き残る」が処理中の手紙には成り立たない。
  runtime が入力証拠を書いた後のクラッシュなら入力は記憶側に残るが（marker は unresolved）、返答も返信の転送も起きない。
- 直し方の案: pop を処理完了まで遅らせる（`inbound` の先頭に `in_flight` を付けて保存し、`answered`/`turn_failed` 時に取り除く。起動時は `in_flight` を再処理する）。
  処理側の重複に備え、ターン要求へ envelope id を渡して runtime 側で冪等にする。

### 欠陥2: C0 レーンの遷移と canonical narration が食い違ったまま残る（形 b）

**修正済み（`activate`・`rollback`・`propose_param`）。reflection / replay の narration は範囲外として残る。**

- 再現（修正前）: `c0_activation_applied` の追記だけを SQLite trigger で失敗させる。
  結果は、レーンは移動済み、narration は0件。初回は `Err(Evidence(Backend…))`、trigger を外した再試行は `Usage("… is not pending")`。
- 影響（修正前）: 自己モデル・運用パラメータの変更（C0 head の移動）が canonical evidence に記録されない。
  呼び出し側は失敗と受け取るが、状態は変わっている。現実の発生要因は、2つのコミットの間のプロセス停止、SQLite の busy timeout、ディスク満杯など。
- 直し方: narration の追記をレーンの書き込みと同じ SQLite トランザクションに入れた（案 A）。
  store に `c0_apply_activation_narrated`・`c0_rollback_narrated`・`c0_insert_proposal_narrated` を足し、レーンの書き込みの後、同じトランザクションで通常の証拠追記と同じ検査（nil id、個体の存在、同一 id の再追記、読み戻し）を通して narration を追記してからコミットする。
  これらは canonical evidence の追記を含む書き込みなので、証拠の追記と同じく `BEGIN IMMEDIATE` で始め、別の書き手がいれば busy timeout の範囲で待つ。
  narration が失敗すれば何もコミットされない。proposal は pending のまま head も動かないので、再試行はそのまま通り、両方がそろう。
  narration の内容（kind、origin class、`source_id`、payload のキー）と trace の記録は従来と同じ。schema・migration は変えていない。
- 照合して補う案（案 B）は採らなかった。原子的にできる以上、食い違いの状態そのものが生じない方が単純で、照合の読み手・起動時の走査・二重書き防止の鍵が要らない。
- 確認: `env -u HAI_API_KEY cargo test -p kamimusuhi-runtime --test write_fault_shapes`。
  `c0_lane_and_its_canonical_narration_agree_after_a_failed_narration`（`#[ignore]` を外した）に加え、失敗した初回が何もコミットしないこと、`rollback`・`propose_param` に同じ注入をしても食い違わないこと、正常系で各遷移の narration がちょうど1件で形が従来どおりであること、別接続が書き込みロックを持っていても待ってから遷移と narration をそろえてコミットすることを確かめる。
- 範囲外: `c0/reflection.rs` の `narrate`（reflection 記録と、reflection 由来の proposal の intake narration）は同じ順序のまま残る。
  このファイルは identity manifest の対象で、直すと常駐への配備に明示承認が要るため、今回は触れていない。
  `c0/replay.rs` の `narrate` 呼び出しはテスト内の fixture だけで、本体の経路には無い（replay の proposal は修正済みの `propose_param` を通る）。
  残るリスクは、reflection 由来の proposal が `improvement_proposal` の narration 無しに pending で残りうること。そこから先の `activate` は上記の修正で narration とそろう。
  `narrate` の links（`DerivedFrom` など）が証拠本体と別の書き込みである点も同じく残る。

## 観察（欠陥とは判定していない）

- `state_record_usage` の `mark_used` は、ツール証拠・発話の保存より前に走る。後段が失敗したターンでも使用回数が増える。
  ただし、参照した生成候補は先に SystemEvent として保存されており、canonical 内容は変わらない。影響は想起の温まり順に限られる。
- intercom の `persist` は `atomic_write` の失敗を無視する。`accept` は保存に失敗しても `accepted:true` を返す（静的確認のみ）。
- intercom の受信処理は、5xx を1回だけ再試行し、それ以外の失敗では `turn_failed` を journal に書いて捨てる。
  明示的な方針として記録は残る。

## 確認したことと未確認

確認した（このリポジトリのテストで、最終状態の読み出しにより）:

- 上の表に **太字** で挙げた新規テスト5本と、ignore 付き再現テスト2本の結果（ignore の2本は `--ignored` で失敗＝再現することを確認）。
- 既存テストの名前と内容は、ファイルを読んで照合した。

未確認:

- 実機での電源断。fsync 後の耐性は既存の子プロセス abort テストまでで、OS・ストレージ層の挙動は見ていない。
- NAS への実配送、NFS の部分書き込み。spool はローカル test のみ。
- 実モデル・実 API を使ったターン。対話は `MockLanguageProvider` と台本 runtime。
- intercom の relay 経路、`defer_delivery` の配達検証経路への障害注入。
- C0 narration 以外の `narrate` 呼び出し（rollback・reflection・replay）での再現。
- trace sink 自体の書き込み失敗時の挙動。
