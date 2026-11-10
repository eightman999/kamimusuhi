# Cycle 21: worker の実行予約と再開の競合

作成: 2026-11-10 UTC。これは実装前の独立設計監査である。読取対象は公開復旧点 `3ee6638` を継ぐ `0ac2643` と、同作業ツリーで復元中の cycle20。以下の新しい ledger、marker、試験は未実装・未実行。Cargo、外部サービス、本番 resident は使っていない。

## 1. 目的と今回の保証範囲

同一の engineering 状態、job、action sequence に対して、協調する新しい worker が workspace 操作へ進む回数を最大 1 回にする。旧 worker の claim 検査直後に resident が再起動して resume する競合と、同一 claim の並行・逐次の二重実行を閉じる。

job ごとの安定した OS lock を、worker の claim 検査前から workspace 操作の終了まで持つ。同じ lock を親の claim と resume にも使う。さらに、実行前に sequence の予約を永続化し、lock 解放後の重複を拒む。ledger は「予約した」という記録であり、実際の開始・成功・子孫停止の証明にはしない。

今回も自動再実行、結果の自動採用、全子孫回収、OS sandbox、敵対的な同一 UID、分散 filesystem の fencing は保証しない。新旧 binary の混在と状態ファイルの巻き戻しには別の制約がある。

### 読み取った接続点

| 場所 | 現状 | 最小の接続 |
|---|---|---|
| [engineering.rs](../../crates/kamimusuhi-resident/src/engineering.rs) `tick_with_runner` | `store.update` で pending/cursor を保存してから runner を呼ぶ | Run/Verify/Finish の claim 前に job lock と ledger 検査を置く |
| 同 `handle_inner` resume | pending が無く Paused/Ready なら再開 | 同じ job lock を取得し、checkpoint 保存の完了まで保持 |
| 同 `load` | 全件 validate 後、途中 action を Paused に回復 | 回復時には ledger を初期化・消去せず、resume を lock で止める |
| 同 `pause_unknown_action` | 対応する pending だけを外し Paused、actions を増やさない | ledger の不確実性もこの経路へ接続 |
| [worker.rs](../../crates/kamimusuhi-resident/src/engineering/worker.rs) `verify_claim` / `execute` | 保存済み claim を読むが、照合後の開始は排他されない | job lock 内で ledger、exact claim、予約、実行を一体化 |
| 同 `ReplyResult` / `decode_reply` | wire は Completed/Rejected、親の Dispatch にだけ Unknown がある | 型付き Unknown receipt を追加し、正常 process exit でも Unknown を保持 |
| [lease.rs](../../crates/kamimusuhi-resident/src/engineering/lease.rs) | 専用 inode、非 blocking lock、leaf の symlink/hardlink 拒否、Drop で明示 unlock | 同じ実装を job lock に使い、Busy と取得失敗を型で分ける |
| [checkpoint.rs](../../crates/kamimusuhi-resident/src/engineering/checkpoint.rs) | schema2、単調 cursor、全件の意味検査 | optional marker の値を全 phase で検査する。検査自体に I/O を入れない |
| [tests.rs](../../crates/kamimusuhi-resident/src/engineering/tests.rs) `InlineRunner` | workspace を直接呼ぶ | process/IPC だけを省く共通 admission を通す |

## 2. 保存するもの

private な `engineering/execution.rs` 相当の小さな module に namespace、job lock、ledger の読取・保存をまとめる。worker の wire codec や親の StoreState と、実行予約の責務を混ぜない。

### checkpoint の optional marker

具体案は `Job.execution_ledger_version: Option<u32>`。旧 checkpoint の field 不在は None、新しく導入した job は Some(1)。None の serialization は省略してよい。Some(0)、未対応の版、旧 schema1 と marker の不整合は、ほかの意味違反と同じく全件 validate の段階で quarantine する。

None は「まだこの保存契約を導入していない」の意味だけに使う。Some(1) の ledger が消えていても None に戻さず、空 ledger を再生成しない。単なる bool を使う場合も、false を第二の正常 marker として増やさず、移行状態を一意にする。

この marker を導入するだけなら checkpoint の schema2 を維持できる。ただし旧 resident の Job は未知 field を無視するため、旧 writer への downgrade を機械的には防げない。schema2 維持案の保証対象は新しい binary に統一した運用である。旧 writer も必ず拒否したい場合は、checkpoint schema の更新と旧版からの明示的な migration を別途含める必要がある。

### job の安定 lock と ledger

namespace は canonicalize 済みの checkpoint 親 directory と、checkpoint の basename 全体から導く。例: `current_state/engineering.json.executions/<job_id>.lock` と `<job_id>.json`。拡張子を捨てて別 checkpoint と衝突させない。job_id は既存の ASCII 英数字・`-`・`_`、最大 80 bytes を共通で検査する。

- lock は一度作った pathname/inode を置換・削除しない。
- 親の legacy 初期化だけが namespace/lock を新規作成できる。worker と marker 導入済みの親は既存 namespace/lock を必須とし、消失を黙って再生成しない。初回の genesis 保存時の directory sync は、先に作った lock entry も対象にする。
- ledger は同じ directory の temp から atomic rename して更新する。
- namespace は candidate worktree の外に置く。model/Request から lock/ledger の任意 path を渡せないようにする。
- 親と worker は同じ namespace 関数を使う。単なる `std::path::absolute` では alias を畳めないため、既存 checkpoint 親を canonicalize し、alias は同じ namespace に収束させるか拒否する。
- namespace directory の symlink・非 directory を拒否する。現 lease は leaf を確認するだけなので、これを確認済みとは扱わない。
- 非協調的な directory の移動・lock の削除・同じ状態の別場所への clone までを、この pathname による排他で防げるとは扱わない。

ledger の例:

```json
{
  "schema_version": 1,
  "job_id": "eng-example",
  "candidate_digest": "sha256:...",
  "high_water": 0,
  "request_digest": null
}
```

小さい有限長（例えば 4 KiB）、`deny_unknown_fields`、regular file、leaf の symlink/hardlink 拒否を契約にする。開く前後の identity を検査する。破損・未知版・identity 不一致・過大な file は保存したまま Unknown にする。

`candidate_digest` は版を含む JSON と固定の Candidate（path、branch、base_commit）から `kamimusuhi_core::digest::json_digest` で作る。現在の HEAD や mutable な検証設定は入れない。`request_digest` は実際に worker が受け取る版、canonical state identity、job、sequence、candidate、exact claimed_action、operation、policy、wire limits を同じ方法で hash する。argv の生値を ledger に複製しない。digest は認証や秘密化ではない。

genesis は `high_water == 0 && request_digest == null`。正の high_water は有効な SHA-256 digest を必須にする。sequence の隙間は正常である。Read/Write/Checkpoint も cursor を消費するため、受入条件は `request.sequence > high_water` であり、`high_water + 1` ではない。最大値への加算、値の巻き戻し、成功後の消去は不要である。

## 3. lock の取得順と Busy の意味

親の順序は `tick_gate（tick のみ）→ job lock → store.inner`。resident の global engineering lease は既存どおり全 lifetime に保持する。resume は job lock→store.inner。worker は job lock→bounded な checkpoint/ledger 読取で、親の global lease を取りに行かない。

`store.inner` や `running` の mutex を持ったまま job lock を待たない。process 終了待ちや model 推論を job lock 内に置かない。すべて非 blocking で取得する。

現在の `lease::acquire` は Busy と I/O/identity failure を String にまとめている。文字列一致で分類せず、`AcquireError::{Busy, Unavailable(...)}` のような型を追加する。既存 `acquire` の String API を wrapper として保てば、resident 全体の caller を大きく変える必要はない。作成可能/既存のみのモードも同じ取得処理で表し、後者では `create_dir_all` と `create(true)` を行わない。失敗後に別の lock を開き直して分類する実装にはしない。

| 場面 | Busy | lock/ledger の不整合・取得失敗 |
|---|---|---|
| 親の新しい claim 前 | defer。pending/cursor/decisions/actions/interruptions を変えない | 実行せず Paused/診断。単なる Busy と区別 |
| 親の resume | bounded なエラー、状態変更なし | 再開しない、記録を保持 |
| worker の admission | Unknown | Unknown |

claim 前の defer は model failure や worker timeout に数えない。選択された一つの busy job によりほかの job が毎 tick 飢餓にならないかも確認する。必要なら有限の選択スキップや in-memory の再試行時刻を使い、defer のために durable action を捏造しない。

## 4. 親が claim を保存する手順

Run/Verify/Finish ごとに、初回だけでなく毎回同じ job lock を取得する。

1. 推論結果を受け取り、pause/cancel を既存どおり確認する。
2. job lock を try-acquire する。Busy は未 claim のまま返す。
3. 現在の job を再確認し、Active、pending 無し、candidate identity、cursor を検査する。
4. 下表に従い genesis を導入、または既存 ledger の identity と単調性を検査する。正の high_water が次の cursor 以上なら rollback/不整合として停止し、cursor を勝手に飛ばさない。
5. 同じ `store.update` で marker と pending/cursor/decisions を更新し、checkpoint の file sync と directory sync の成功を待つ。
6. job lock を解放してから worker を spawn する。lock を持ったまま spawn/dispatch すると、自分の worker を Busy にする。

marker の初回導入と pending の保存は同じ checkpoint transaction に含める。ledger の genesis はその前に durable にする。親が途中で pause されて claim を作れなくても、残った genesis は後で同じ identity を確認して採用できる。

| marker | ledger | 親の扱い |
|---|---|---|
| None | 無い | job lock 内で durable な genesis を作り、その後 marker+pending を保存 |
| None | 同じ job/candidate の genesis | 導入中断として採用可能 |
| None | 正の high_water | 自動採用不可。通常の bootstrap 順序では発生しない |
| Some(1) | 正常・同じ identity | 単調性を検査して次の claim へ進む |
| Some(1) | 無い | 消失として停止。空から作り直さない |
| 任意 | 破損・未知版・別 identity | 元 file を保持して停止 |

claim 前の失敗には、まだ `pause_unknown_action` に渡す pending がない。`last_error` と sequence 0 の admission 診断で Paused にするなど、未 claim の専用経路を使う。ledger の高い sequence を通常 observation や interruption の sequence に転記すると、checkpoint の cursor 不変条件を壊す。未実行の新しい判断を actions/interruption として数えない。

## 5. worker の admission と receipt

推奨順序は次のとおり。

1. 有限長の frame、request の構造、版、policy/limits を検査する。
2. 共通 namespace の job lock を取得する。取得できない場合は Unknown。
3. bounded ledger を読み、identity、形式、既存予約を検査する。
4. `sequence <= high_water` なら digest が同じでも違っても Unknown。過去の予約を、現在の単なる claim 不一致より先に判定する。
5. checkpoint 全体の意味を検査し、同じ job、marker、candidate、Active、pending sequence/action を確認する。
6. cancel が既に立っている場合は、未予約・未実行として戻す。
7. 高水位と request digest を durable に保存する。保存の確実性が得られなければ実行しない。
8. job lock を保持したまま workspace 操作を行い、operation の確定結果を返す。取消・エラー・成功のどれでも予約を戻さない。

worker は marker/genesis を初期化せず、checkpoint を更新しない。Finish の summary と Ready の更新は親のまま、worker の操作単位は Run または Verify のままでよい。job lock は少なくとも workspace 関数が終了するまで保持する。返答の送信が遅れても ledger が逐次 duplicate を止める。

`ReplyResult::Unknown { reason }` を追加し、既存の version/job/sequence/operation と有限 byte 検査の後に `Dispatch::Unknown` へ変換する。Unknown を receipt の正常 exit で Completed に読み替えない。

| 状態 | 結果 | 親の状態 |
|---|---|---|
| この request の構造不正、正常 ledger の下で未予約の exact claim 不一致、予約前取消 | Rejected | 既存の確定失敗 observation |
| 同じ/古い sequence の予約、digest conflict、busy lock、marker/ledger 不整合、読取不能 | Unknown | 対応 pending だけを外し Paused、actions は増やさない |
| ledger publish/sync の失敗で予約の永続性が不確実 | Unknown | 実行せず Paused、後続で空に戻さない |
| workspace 関数が結果を返した | Completed(Result) | 既存どおり成功または確定失敗を記録 |
| transport loss、worker 異常終了、receipt identity 不一致 | Unknown | 既存の経路を保持 |

正常 ledger と有効な snapshot の下で「未予約の claim が既に無い」と確認できたケースと、checkpoint の読取・decode 自体が不能なケースは区別する。後者を一律 Rejected にしない。

wire の `VERSION` は 2 への更新を推奨する。旧 worker は version1 の新 request を解釈して ledger を無視できるためである。新しい親は旧 receipt を Unknown にし、新しい worker は旧 frame を拒否する。ただし、既に version1 request を受信した旧 worker をこれで停止させられるわけではない。

## 6. resume・回復・取消

resume は保存済み job の identity を取得し、job lock を取得してから `store.update` に入る。その中で pending 無し、Paused/Ready、marker/ledger の整合、cursor の条件を再確認し、phase/validation/next_step の変更が directory sync まで成功した後に lock を解放する。

legacy の resume は None+ledger 無し、または同 identity の genesis を許容してよいが、ここでは ledger/marker を導入しない。導入は次の worker-target claim に限定する。None+正の高水位や、marker 有りの消失・不整合は resume でも止める。

これは worker の「exact claim 検査→開始」と resume を直列化する。親の claim も同じ lock を使うことで、resume が Active を publish してまだ lock を持つ短い間に、次の worker が発行され Unknown になる競合を防ぐ。

`load` の途中 pending の回復は job lock 無しで継続してよい。実行権を取り消して Paused にする操作であり、ledger を消去せず、続く resume が生きた worker に阻まれるためである。load で ledger の初期化・移行・高水位更新を始めない。marker がある job の read-only な表示で I/O を追加しない。

`pause` は job lock を待たず、既存の phase/cancel を更新する。worker は stdin EOF、余分な control 入力、既存期限を使って取消される。取消が予約前なら操作 0、予約後なら操作 0 の可能性があっても予約は残る。実行開始済みの取消は既存 workspace/process の確定結果と transport 結果を使う。

旧 binary の worker は新 lock を知らない。旧 schema/marker 無しの途中 action は引き続き Paused とし、既存の「以前の実行が停止し、効果を確認した後に明示 resume」の境界を残す。新 lock を一度取れた事実を、旧 worker や escape した子孫が無い証明にはしない。

## 7. ledger の atomic 保存と crash の扱い

[util::atomic_write](../../crates/kamimusuhi-resident/src/util.rs) は PID 固定 temp を `File::create` し、directory sync を含まない。ledger 用にはそのまま使わない。

保存 helper は job lock を要求し、以下を順に行う。

1. canonical namespace 内の短い一意な sibling temp を `create_new(true)`、mode 0600 で作る。衝突時の再試行も有限にする。
2. 小さな上限内の ledger 全 bytes を書く。
3. temp の `sync_all` を成功させる。
4. 正規の ledger pathname に rename する。
5. namespace directory の `sync_all` を成功させる。

namespace を新しく作った場合は、その親 directory も sync してから checkpoint marker を publish する。既存の lock/ledger と directory の identity を確認し、symlink や hardlink を上書きして「修復」しない。失敗時に掃除するのは自分が作った temp に限る。再起動時の orphan temp は最新 ledger として採用しない。

| crash/失敗の位置 | 残り得る状態 | 次回 |
|---|---|---|
| genesis temp 作成中 | marker 無し、正規 ledger 無し | 同 lock の下で通常導入可能。orphan は無視 |
| genesis durable 後、checkpoint 前 | marker 無し、genesis | 同 identity だけ採用 |
| pending 保存後、worker 予約前 | marker+pending、genesis または前の高水位 | resident は既存どおり Paused に回復。自動 replay しない |
| 高水位 rename/sync の途中 | 旧または新 ledger、永続性不確実 | 当該 worker は実行しない。自動 replay しない |
| 高水位 durable 後、workspace 呼出し前 | 正の予約、効果 0 の可能性 | 同 sequence は Unknown。予約を巻き戻さない |
| 効果後、receipt/outcome 保存前 | 正の予約、効果不明 | 同 sequence は Unknown、親は Paused |
| 完了後 | 正の予約、親の確定結果 | 同 sequence の重複を止め、より大きい新 claim は許す |

checkpoint と ledger は二つの file なので、単一 transaction と同じ原子性はない。保存順序によって「予約されたが実行されなかった」を許し、重複効果を防ぐ。process kill 試験だけで、停電・storage firmware・network FS に対する一般的な耐久性を実証したとは書かない。

## 8. unit test の InlineRunner を同じ境界へ接続

worker の admission を transport から独立した private 関数にし、stdio worker と `#[cfg(test)] execute_inline` が同じ関数を呼ぶ。InlineRunner は Request::new に state_path/claimed/policy/limits を渡し、その helper を呼ぶ。lock、marker、exact claim、ledger 予約、workspace 実行は省かない。省くのは `current_exe` の spawn と IPC だけである。production の inline fallback は追加しない。

[worker_dispatch.rs](../../crates/kamimusuhi-resident/src/engineering/tests/worker_dispatch.rs) の合成 ResultRunner は親の receipt 反応・保存失敗を検査する seam として残せる。その試験を実 worker、ledger、process 終了の証明として数えない。

## 9. 最小 fixture と測定

同じ使い捨て Git/worktree、外部 counter、loopback barrier を使う。正常系だけでなく、effect 0 と effect 1 の不確実性を分ける。待機は barrier と有限期限で制御し、狭い race を sleep の偶然に依存させない。

| fixture | 期待する観測 |
|---|---|
| W1 を Run 内の barrier で保持し、同一 claim の W2 を発行 | W2 Unknown、workspace counter は 1 |
| W1 終了後、checkpoint を同じ pending のまま W3 を発行 | W3 Unknown、counter は 1 のまま |
| 同 sequence で argv と checkpoint.pending.action を共に変更 | claim mismatch ではなく ledger により Unknown、追加 effect 0 |
| sequence に隙間を作り、より大きい正規 claim | 受理されて effect が 1 回増える |
| 旧 worker の stdin と barrier を保持、新 resident が pending を回復 | Paused、pending 無しでも resume は Busy。worker 解放後は resume 可能 |
| claim 検査後・operation 前の private test hook で停止 | 同 lock の resume が進まず、release 後にのみ状態変更 |
| durable 予約後・effect 前の模擬 crash | counter 0、再発行 Unknown |
| effect 後の receipt loss/outcome 保存失敗 | counter 1、再発行 Unknown/Paused、Ready を捏造しない |
| marker 無し+genesis、marker 無し+正の高水位 | 前者だけ採用、後者は停止 |
| marker 有り+ledger 消失/破損/未知版/別 identity/過大 file、lock/namespace 消失 | 残る file を保持、操作 0、空 ledger/新しい lock に戻さない |
| symlink/hardlink/namespace alias/安定 lock inode | 既存と同じ leaf 拒否、alias は同じ排他、独立 job は互いに妨げない |
| rename/file sync/directory sync の失敗注入 | workspace 呼出し 0、後続で ledger を巻き戻さない |
| 予約前/後の取消 | 予約前は高水位不変、予約後は高水位保持 |
| 旧 wire 版/未知 marker、Busy と permission failure | 型ごとの結果が一致し、Unknown を Rejected に丸めない |

[実 worker fixture](../../crates/kamimusuhi-resident/tests/engineering_worker.rs) の現 stdout/stderr は `worker-{sequence}.out/.err` である。同 sequence の並行試験前に attempt ID を加える。そうしないと同じ file を truncate し合い、receipt の結果が信用できない。

同 fixture の `request()` は現在 snapshot を毎回書く。ledger genesis を毎回書き直す helper にしてはならない。初回準備と新しい pending の保存を分け、ledger が正の値になった後も保持することを fixture 自身で確認する。worker は checkpoint bytes を変更しない既存検査も維持する。

記録する前後指標は workspace counter、worker 起動数、Unknown/Rejected、claim/cursor、actions/interruptions、resume 成否、停止までの時間。まだこの設計の実行値はない。静的に見つかった二重開始の余地を、実行済みの再現とは記載しない。

## 10. 残る穴と実装時の確認事項

- stable lock だけでは終了後の重複を止められない。高水位だけでは「検査→開始」と resume を直列化できない。両方を同じ admission で使う。
- marker 有りの NotFound を初回と扱う、marker 無しの正の ledger を採用する、成功/取消時に高水位を消す、のいずれも行わない。
- worker の SIGKILL 後に lock は解放されても、逃げた子孫が残る可能性はある。lock 取得を全子孫停止の証拠にしない。明示 resume の既存確認は必要である。
- marker と ledger の両方を過去へ巻き戻せば検出できない場合がある。backup/復元の単位には checkpoint と execution namespace の両方を含め、コピーした状態を同時起動しない。
- hidden worker entry と digest は同一 UID からの偽 request を認証しない。この設計は配置済み binary と保存 directory を共有する協調 process 間の整合性である。
- checkpoint の global job 数は増え続ける一方、worker の snapshot 読取には 64 MiB 上限がある。この成長問題は別 backlog とし、今回の ledger file 上限と混同しない。
- ledger 状態を status/list/show のために全件読まない。純粋な過去 receipt 表示と、実行前・明示 resume 時の admission を分離する。
- 高水位不整合で cursor を修復する、自動的に pending を再作成する、過去結果を推測して Ready にする処理は今回追加しない。

実装を分担するなら、保存 module/lease の型付き失敗、親の marker/claim/resume、worker admission/codec、実 process fixture の四境界でファイル所有を分ける。まず duplicate と resume の fixture を作り、変更前の現象を実測し、その後に同じ条件で比較する。
