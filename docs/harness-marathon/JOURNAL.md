# エージェントハーネス改良記録

## 実施条件

- 開始: 2026-11-09 01:06:48 JST / 2026-11-08 16:06:48 UTC。
- 起点: `master` の `f31aae8f612a20bc695c1b854661306b90d79217`。
- ブランチ: `exp/harness-marathon-20261109`。マージしない。
- 実測の累計24時間を目標とする。中断時間や並行作業を水増ししない。
- 本番 resident、Mac/Pi、Slack/Discord、メール、intercom relayへ接続しない。
- API鍵を追加せず、使い捨てディレクトリ・loopback・モックで検証する。
- 外部原文・長い引用入り成果物はignoreし、公開する調査記録は自分の言葉とリンクに限る。
- 既存の未追跡ファイルは触らず、変更したパスだけをstageする。

## Cycle 0 — 新規開始とベースライン

時刻: 2026-11-09 01:06 JST開始。

本当の目的: 中断後も同じブランチと検証状態から再開でき、既存失敗と変更による失敗を区別する。
前提と確かめ方: clone済み・Rust導入済みという前提を、`pwd`、`git status`、道具の存在確認で調べる。
一番安い確認方法: 変更前のHEAD・作業ツリーを記録し、指定されたCIスクリプトをそのまま1回実行する。

確認した事実:

- 提供cwdは空でGit作業ツリーではなかった。GitHubの認可済み接続で公開repoとpush権限を確認後、`master`を深さ30で取得した。
- `docs/harness-marathon/STATE.json` と既存 `exp/harness-marathon-*` ブランチは無く、新規開始。
- `cargo` / `rustc` / `rustxng` / imozuru は未導入だった。
- `rust-toolchain.toml` 指定のRust 1.92.0とrustfmt/clippyをrustupで導入。`cargo --version`・`rustc --version`: PASS。
- imozuruを指定URLからcloneし、`$HOME/.agents/skills/imozuru/`へ導入。SKILL.mdを通読。
- 指定の `docs/primary-agent.md` は現HEADに存在しない。他の指定文書は存在し、ハーネスに関する部分を確認。
- 調査役2名（ハーネス一次情報、RustXNG契約）と、実装境界の読み取り専用調査役を並行起動。ファイル編集は担当を分離する。

| 検証 | 結果 | 確認した層 |
|---|---|---|
| `git log --oneline -20` / `git status` | PASS | 起点と履歴、既存の未追跡物 |
| `cargo --version` / `rustc --version` | PASS | Rust 1.92.0実行 |
| imozuru SKILL.md存在と読取 | PASS | 導入と手順確認 |
| `bash scripts/ci-local.sh` | FAIL (exit 101) | fmt PASS後、clippy段階で終了。保持ログに直接の診断が無いため原因未確定。build/testはSKIP |

コミット: この節を導入する `chore(harness): 長時間改良の再開記録を初期化`。

### 保存経路と中断の記録

- 初回CIは16:11:17〜16:15:53 UTCに実行。変更前の失敗として保持し、変更による退行と混同しない。
- 通常の `git push` はcredential未設定で失敗。新しい鍵は作らず、既存GitHub connectorのGit Data APIでtree/commitを作成し、`force:false`と期待HEAD付きで専用branchを更新した。
- 初回の公開commitは `aba0a9d431afd54aee975253df328815c77acfd4`。ローカルcommit `35e6c6b` とtreeが一致することを検証してlocal refを同期し、元local commitも別refへ保持した。ファイルは破棄していない。
- connector準備後にcatalog変更で待機が中断された。最後の実作業ログは16:44:36 UTC、親の再開は22:30:27 UTC。間の空白は累計時間に入れない。
- 原因未確定のbaseline失敗を切り分けるため、起点 `f31aae8` の別worktreeで同じCIを再実行する。

## Cycle 1 — HTTP statusによるtool結果の誤成功を修正

時刻: 再現・修正は2026-11-09 01:43〜01:44 JST、記録の統合は07:30 JST以降。

本当の目的: upstream/プロキシが失敗した呼出しを成功の証拠として主推論へ渡さない。
前提と確かめ方: `ToolServerConfig::call` がHTTP statusを見ずJSONのokを信用する点を、7種の非2xx fixtureで再現する。
一番安い確認方法: 実loopback HTTPで失敗status＋成功形JSONを返し、ToolCallRecord.okと実呼出し回数を見る。

開始前の確認方針は担当への委譲メッセージに記録した。親の接続待機により、このJOURNALへの転記は実施後になった。

変更: JSON parse前に `response.is_success()` のguardを追加。失敗本文をtool結果へ取り込まない。HTTP200の構造化失敗は従来通り維持。

| 指標（同じfixture） | 前 | 後 |
|---|---:|---:|
| 非2xxの誤成功 (302/400/401/403/429/500/503) | 7/7 | 0/7 |
| HTTP200成功/構造化失敗の対照 | 2/2 PASS | 2/2 PASS |
| 非2xxごとの実HTTP呼出し | 1 | 1 |
| 非2xx本文の不反映 | 未保証 | 7/7 PASS |

| 検証 | 結果 |
|---|---|
| 新テストのみの `cargo test --offline -p kamimusuhi-persona-http tools::tests::http_error_statuses_are_not_successful_tool_results -- --exact --nocapture` | 期待したFAIL (exit 101) |
| 修正後 `cargo test --offline -p kamimusuhi-persona-http` | PASS、64件 |
| `cargo clippy --offline -p kamimusuhi-persona-http --all-targets --all-features -- -D warnings` | PASS |
| 対象file rustfmt / `git diff --check` | PASS |

数字は9件のHTTP境界fixtureについての実測。実モデルの総合成功率ではない。
コミット: この節と変更を含む `fix(tools): HTTP失敗をツール成功として記録しない`。

## Cycle 2 — RustXNG sidecar（着手前）

本当の目的: residentから検索でき、検索プロセスの障害が主loopの停止や権限の拡張にならないようにする。
前提と確かめ方: 最新release v0.31.0の契約はform POST。checksum・help・実health/検索を確認し、製品コードはmockで独立検証する。
一番安い確認方法: literal loopback fixtureでmethod/content-type/timeout/不正応答を観測する。

所有: `web_search.rs` は検索担当、config/state/tools/承認は親。上流RustXNGのコードは取り込まずCLI/HTTPのみ。

## Cycle 3 — imozuru手順をresidentに導入（着手前）

本当の目的: 調査を文章の指示だけにせず、原文・追跡・停止根拠を再開可能な状態として残す。
前提と確かめ方: (a)SKILL互換ローダは汎用性があるが、手順の信頼と追加権限という新しい契約が増える。(b)専用toolなら固定actionと既存のgateで狭く実装できる。
一番安い確認方法: 一時root＋fake fetch scriptで保存/再読込/停止条件と私設宛先・path拒否を検証する。

選択: (b)。operatorが導入したimozuru scriptsを固定argvで呼び、手順自体はtyped stateとして実装する。任意skillの命令文やallowed-toolsを権限として取り込まない。
所有: `research.rs` は調査担当、公開設定/権限/帰属は親。

## Cycle 4 — 未完了タスクを履歴間引きから保護（着手前）

本当の目的: 長時間走っているtask/commitmentが通常toolの履歴増加で消えないようにする。
前提と確かめ方: persistの先頭drainが状態を見ないため、古いactive項目も削除される可能性を再読込fixtureで確かめる。
一番安い確認方法: active1件→上限を超える完了履歴→reloadという同じ条件で前後の保持数を比較する。

所有: `tasks.rs` / `commitments.rs` は保持機能担当。他ファイルは変更しない。

### Cycle 4 の結果

担当の2件の回帰testで、旧実装が未完了task/commitmentを消すことをFAIL (exit101)で確認した。担当が中断された時点ではtestのみ保存されていたため、親が既存の `finished()` / `terminal()` 判定で終端履歴だけを間引く最小修正を完成させ、同じtestを再実行した。

| 指標（履歴上限超過→保存→再読込） | 前 | 後 |
|---|---:|---:|
| 未完了task 4状態の保持 | 0/4 | 4/4 |
| 未完了commitment 3状態の保持 | 0/3 | 3/3 |
| 完了task履歴の上限 | 400件（未完了も削除） | 400件（未完了は別に保持） |
| 終端commitment履歴の上限 | 200件（未完了も削除） | 200件（未完了は別に保持） |

- `cargo test --offline -p kamimusuhi-resident --lib retention_preserves_unfinished`: PASS、2/2、exit0。
- 対象2fileのrustfmt: PASS。
- 未完了件数そのもののadmission上限は、この変更の対象外。既存ID・checkpoint・状態遷移を保持する。
- コミット: `fix(resident): 未完了作業を履歴の間引きから保護`。

## 中断時の確定状態 — 2026-11-09 07:36 JST以降

- tool実行がユーザーにより中断され、3担当ともinterruptedになった。新しい改良cycleは開始せず、既に保存された変更と引継ぎを固定した。
- RustXNG v0.31.0: `.sha256` とGitHub asset digestの両方に一致。SHA256 `1f1dad6cf79415b3a52a23ae950af90f77d3650afef8d0d2015773af8f7ec8de`。実 `--version` / `--help`: PASS。
- `--profile full -b 127.0.0.1 -p <一時port> -c deploy/resident/rustxng-full.toml`: health HTTP200/OK。GETとform POSTの `/search` は両方HTTP200のJSON。queryは `agent harness long running checkpoint resume`、両方0件・不応答9engine。約2.00秒/2.01秒。起動したchildは終了・wait済み。
- `full`単独は上流のbackoff無効を意味しない。設定overlayで既知のban/suspended/empty-result条件を0にし、limiter=falseにした。未知の上流suspension経路や実エンジンの全挙動を検証済みとはしない。
- residentのweb_search/researchモジュールは未保存・未接続。設計APIと統合先はRESUME.mdに残した。
- 元HEADのCI再試験はfmt/clippy/provider-worker buildまでPASS。workspace testで `agent_exec::executor::tests::an_attached_server_is_started_reused_and_reaped` の既存FAILを観測。その後の中断でセッションとプロセスが消え、最終終了コードが無いため全体はSKIP（中断・既存FAIL観測）とする。
- imozuruの直接取得はDNSでFAIL。Web抽出本文の保存は成功したが、全文保存・1周完了・飽和は未達。
- 24時間未達。REPORT.mdは作らず、STATEをpaused_interruptedとして保存する。

### 公開commitと最終checkpoint

- 初期記録: `aba0a9d431afd54aee975253df328815c77acfd4`。
- HTTP status境界: `785f2493a73d9c864f805d41ff2203d1cb5f089c`。
- 未完了作業の保持: `9049783f0cd05d53cedcd8f297c67d73c14dfb2a`。
- 最終 `cargo fmt --all -- --check` と `git diff --check`: PASS。
- 計測値と実行済み/未実行の区別を `evaluation/harness/marathon-20261109.json` に保存した。
- 最後のcheckpoint commitにはBLOCKED/RESUME/map/logとRustXNG設定overlayを含める。取得原文・raw応答・長い引用は含めない。

## 再開と目的の更新 — 2026-11-09 07:51 JST

- 最新 `741f2d0` とSTATE/JOURNAL/BACKLOG/BLOCKED/log20を確認。未追跡の既存成果物は保持。累計0.807778時間から再開し、中断時間は加算しない。
- ユーザーがskills/検索tool改良ではなく、resident自身のエンジニアハーネスを要求。RustXNG/imozuru統合は保留し、主推論→編集→実検証→修復→再開を優先する。個々の処理の期限・取消・明示workspaceは維持し、仕事全体の固定試行回数で終了しない方式を検討する。

## Cycle 5 — residentのnative engineering agent（着手前）

本当の目的: 外部coding CLIを必須にせず、residentが開発目標を保持して自ら判断・実装・検証・修復を続ける。
前提と確かめ方: self_improveは単発start/status/applyで検証失敗のrepair loopが無い。commitmentsのcommand checkは元workspaceを対象にする。現在コードを読み、同じ候補を実検証する経路を新しい状態機械で確認する。
一番安い確認方法: モデルの判断をscripted fixtureにし、一時git/worktreeと実検証コマンドで『誤った成功申告→失敗→修正→合格』を実行する。

設計: 専用のnative agent stateをresidentが所有。既存routerで1判断ずつ取得し、actionを実行前に永続化。隔離worktree、CAS書込、実終了コードによる検証、検証対象HEAD、短い作業記憶、再開cursorを保持する。Readyは検証済み候補で、自動merge/push/deployしない。全体の固定turn数は設けず、停止/再開をoperator入口に残す。既存task plane/commitmentsを廃止しない。

担当: engineering.rsと設定/HTTP/CLI統合はroot、engineering/workspace.rsはharness_audit、engineering/tests.rsはresearch_harness。

## Cycle 6 — native agentの子process期限と出力回収（着手前）

本当の目的: 検証・実行processが出力を続けてもagentがcancel/期限を観測し、residentを止めない。
前提と確かめ方: runtime agent_exec/process.rsのstdout受信continueがdeadline判定を飛ばし、line/stderrが無制限に蓄積される経路をfixtureで再現する。
一番安い確認方法: 連続stdout/改行無し/stderr洪水の短いローカル子processを起動し、経過時間・回収量・終了理由を前後で比較する。

担当: process.rsだけをrustxng_contractが編集。他のexecutor/skill/searchは変更しない。

### Cycle 6 の結果

連続stdoutでもchunk/lineごとにdeadline/cancelを確認。stdoutは1行1MiB・合計4MiB、stderrは読取中から末尾16KiBに制限。reader間のqueueは合計約1MiB。正常なJSON行は切断せず、上限を超えた行を除外して次の行から回復する。process groupの停止後、pipeの回収待ちは100msまで。既存TERM grace300msは維持。

| 同じfixtureの指標 | 前 | 後 |
|---|---:|---:|
| 100ms deadline + 1ms/event callbackの連続出力 | 1436ms | 402ms |
| 2MiB異常行のcallback最大サイズ | 2097152 bytes | 異常行を除外、後続tail 4 bytesを保持 |
| 約6MiB出力のcallback総量 | 6290688 bytes | 4193792 bytes |
| stderr 16MiB洪水時の親VmHWM | 17808kB | 2044kB |
| stdoutを子が保持、100ms deadline | 402ms | 403ms |

VmHWMと経過時間はこのコンテナの単回比較で、一般的な性能ベンチではない。

| 検証 | 結果 |
|---|---|
| `cargo test -p kamimusuhi-runtime --lib agent_exec::process::tests -- --nocapture` | PASS 15/15 (0.61s) |
| `cargo clippy -p kamimusuhi-runtime --all-targets --all-features -- -D warnings` | PASS |
| 対象rustfmt/check・git diff --check | PASS |

既存attached-server試験は、PID namespaceと/procの視点が異なり `ps -p child.id()` が孤児を認識できないことを隔離再現。executor.rsは変更していない。process groupを明示的に抜けた子のblocking readerを完全終了させることは今回未保証。callerの待ちは有限。

コミット: `fix(harness): 連続出力でも実行期限と回収量を守る`。

## Cycle 7 — 実resident入口からnative agentを通す（着手前）

本当の目的: 状態機械のfixtureだけでなく、HTTP/CLI・router・JSON行動・常設schedulerが実際につながることを確認する。
前提と確かめ方: native loopのtrait fixtureでは9contractがPASSしたが、model HTTPのプロトコルやdaemon起動はまだ未確認。
一番安い確認方法: 明示的な一時config・loopback fake model・一時gitだけでresident子processを起動し、失敗→修復→Readyとrequest数を測る。

担当: tests/engineering_http.rsをrustxng_contractが所有。他の本番residentやrelayは使用しない。

### Cycle 5 / 7 の結果 — native agent初版

- native loop、durable cursor、working memory、準備/Active/Ready/Paused、CAS編集、実検証repair、既存router接続、HTTP/CLI/主推論からの起動入口を実装。外部coding CLIのtask planeは不要。
- unit contract 12/12、workspace fixture 8/8、実resident+CLI+loopback model integration 1/1: PASS。スキーマと状態だけの試験に加え、git/worktree/check commandは実物で検証した。
- 新設機能のため、未実装だった状態を成功率0として比較しない。次表は今回新しく通した制御契約の実測。

| 実daemonでの指標 | 値 |
|---|---:|
| model HTTP POST | 5回 |
| durable decisions/actions | 5/5 |
| 実検証 | 2回（FAIL→PASS） |
| 修正操作 | 1回 |
| Ready後の実daemon再起動による追加model/検証 | 0/0 |
| 元checkout HEAD/branch/fileの変更 | 0 |

| 新実装内で発見した回帰 | 前 | 後 |
|---|---:|---:|
| failed verify→CAS拒否→正常修正のrepair計上 | 0 (FAIL) | 1 (PASS) |

- 原子Writeは完成済み旧/new内容を保持し、候補外の小さいintent journalで所有tempを回収。temp作成後/new link確定後/rename後の3状態をfixtureで回収した（実process SIGKILL試験ではない）。
- persist前の失敗はcommand0回。command後のpersist失敗はpendingを保持してquarantineし、reload後も重複実行0、成果を保持。
- 実daemonはReady後に停止・再起動して同一候補とreceiptを確認。実行途中のprocess crashと複数resident排他は次cycle。
- `CARGO_INCREMENTAL=0 cargo test -p kamimusuhi-resident --test engineering_http -- --nocapture`: PASS (7.59s)。対象clippyもPASS。増分archiveの環境エラーはincremental無効で解消し、cargo cleanは使わなかった。
- `bash scripts/ci-local.sh` を全workspaceで実行中。fmtはPASS、最終結果は次の記録で確定する。全CI成功とはまだ記載しない。
- 実装と設定方法は `docs/engineering-agent.md`。全体turn数の固定上限は無く、操作ごとの期限・出力枠・取消を残す。

### 時間計測の継続

rootのGitHub tree作成待機と環境更新を挟み、最後の担当clippyログは23:10:55 UTC、root再開は04:32:26 UTCだった。実作業のない空白は加算しない。実測累計1.133847時間から次セッションを続ける。process変更の公開commitは `233f026f91ec388ad024687b08433bd5918d1d7a`、local/remote tree一致を確認。

### 初版の全CIと保存

公開commit `001370fd2220e0f8e4107ce685b784a7588c5842`。local/remoteのtree一致を確認。

- `bash scripts/ci-local.sh`（CARGO_INCREMENTAL=0、build jobs4、offline）: FAIL exit101。fmt PASS、clippy途中で終了。保持ログに直接の診断がなく、変更由来の失敗か環境要因かはこの結果だけでは未確定。後段build/testはSKIP。
- 方法を変え、build jobs1で `cargo clippy -p kamimusuhi-resident --all-targets --all-features -- -D warnings` を実行: PASS exit0（14.44s）。対象crateは通過、全workspaceの成功とは扱わない。

## Cycle 8 — 一つの保存領域の所有とcheckpoint意味検査（着手前）

本当の目的: 同じ目標を二つのresidentが実行したり、破損した完了証拠から継続したりしないようにする。
前提と確かめ方: 現在は同SharedのMutexだけで別process排他がない。loadはpendingを即回復するため、tick前のlockでは遅い。Ready/receiptの意味検査もまだない。
一番安い確認方法: 専用File::try_lockをload前から保持し、2daemonの明示拒否・所有者kill後の再取得と、整形式だが意味の壊れたJSON原本の無変更をfixtureで確認する。

既存runtimeのidentity/idle-cognitionにも同じlock方式があり、Rust1.92のAPIを型検査。overlayfs上の実子processでbusyとSIGKILL後の取得を確認した。atomic置換するcheckpoint自体をlockせず、削除しない専用engineering.lockを保持する。現在の別process unknown-outcome方針は維持。

担当: lease.rsはharness_audit、semantic/回復unit fixtureはresearch_harness、実daemon Active再起動/lease fixtureはrustxng_contract。rootがloadと意味検査を統合する。

### Cycle 8 の結果

専用engineering.lockのFileをload前からstoreの生涯保持。checkpoint自体をlockせず、取得失敗時はpendingの回復にも触れない。OSleaseで保護する範囲はengineeringで、resident全体のboot_epoch/他store起動排他ではない。ロード前の全job意味検査、保存前の意味検査、quarantine時のscheduler停止を追加。正当な日本語メモの切り詰めもellipsis込みbyte枠へ修正。

| 同じfixture | 修正前 | 修正後 |
|---|---|---|
| checkpoint正常系2群 | 2 PASS | 2 PASS |
| checkpoint不整合拒否5群 | 5 FAIL | 5 PASS |
| 長い日本語Finish要約→結果保存 | FAIL（実check成功後に保存拒否） | PASS（Ready/receipt/4096bytes以下を保持） |
| 長い日本語resume note | 前段FAILにより未到達 | PASS（2048bytes以下、再読込で保持） |

意味検査の修正前は各負例群の先頭で停止したため、28変種全てを実行したとは扱わない。修正後は28変種を拒否し、原本bytes/候補/元checkoutを保持、追加model/command0。

- `cargo test --offline -p kamimusuhi-resident --lib engineering:: --no-fail-fast`: PASS 31/31（unit20、workspace8、lease3）。
- 実daemon E2E: PASS 2/2。新caseはAの3回目の推論をchannel barrierで保留し、同rootのBが明示busyでcreate/pause/resume各400、checkpoint不変、推論0。Aをkill/wait後、Bを生かしたままCがleaseを取得し、同じ未コミット成果からReadyへ進んだ。
- 新caseの実測: 推論POST5（A3/B0/C2）、durable decisions/actions4/4、実Write1、実検証1、context actions `[0,1,2,2,3]`。中断RPCはaction claim前なのでdurable decisionに数えない。
- leaseのprimitive試験はLinux overlayfs。NFS/SMBや複数ホストのfencingは未保証。
- 最後にrootがfmt・resident全target/allfeature clippy・実HTTP試験を順に確認し、最終終了コードを記録する。

Cycle8最終確認: `cargo fmt --all -- --check` PASS、`cargo clippy --offline -p kamimusuhi-resident --all-targets --all-features -- -D warnings` PASS、`cargo test --offline -p kamimusuhi-resident --test engineering_http` PASS 2/2。いずれもexit0、CARGO_BUILD_JOBS=1/CARGO_INCREMENTAL=0。コミット: `fix(engineering): 永続状態の所有と再開の整合性を守る`。

## Cycle 9 — 大きな実コードを部分的に読む・直す（着手前）

本当の目的: モデルへ全文を往復させず、大きなRust sourceの必要箇所を調査・修正できるようにする。
前提と確かめ方: I/Oの1MiB上限より先に観測32KiB/context8KiBでRead本文が切れ、全文CASのWriteでは元内容を表現できない。line rangeと一意substring置換で解消できるか確かめる。
一番安い確認方法: UTF8/CRLF/JSON byte枠の改ページ、重なりを含む複数一致拒否をfixtureで先に確かめ、128KiB超のRustfileを実compilerでFAIL→PASSへ修復する。

担当: workspace.rsはharness_audit、rootがAction名/prompt/validation失効/回復へ接続。旧Read/Writeは保持。

## Cycle 10 — モデル応答とprovider障害からの継続（着手前）

本当の目的: 不正なモデル出力や一時的なHTTP障害を、目標の喪失や作業済みの副作用に変えず、次の判断へ戻す。
前提と確かめ方: 不正actionはrouterのHTTP成功後にnative parseで拒否される。単一auto tierのHTTP503は既存routerの60秒benchを経て再候補になる。これらを混同せず実loopback modelで確かめる。
一番安い確認方法: channelで応答を制御し、不正JSON2回→正常actionの実呼出し数と、503→auto再試行の実POST数・durable cursor・候補を検査する。

担当: engineering_http.rsはrustxng_contract。60秒の実retry待ちをテスト用本体変更で短縮せず、実workerを起動したまま期限付きの次request待ちで確認する。

### cycle8凍結版の全CI結果

公開commit `980c564579faa703cf55651c92de3e4e0495f62c` と同一treeの別worktreeを使用。fmt、全workspace clippy、provider worker buildはPASS。resident unit269/269とHTTP2/2もPASS。runtimeの既存attached-server FAILを観測後、wrapperがexit137で終了し最終JSON報告なし。終了理由は未確定。全CIはINTERRUPTED、後段のcore dependency gateは未確認。ログ `logs/cycle8-frozen-ci-20261109.log` はignore対象。

### Cycle 9 の結果

`read_range` と `replace` をnative agentの判断・実行・repair記録・再開処理へ追加。行範囲は完全なJSONで返し、UTF8/CRLFを保つ。置換は重なった一致も拒否し、既存の原子CAS保存を利用する。途中の部分置換は非冪等になり得るので、結果不明ならPausedにする。

| 同じ大source fixture | 既存Read | ReadRange + Replace |
|---|---|---|
| source | 364,849 bytes | 同じsource |
| 修正対象 | truncatedで未取得 | 完全な6,109-byte JSONで取得 |
| 継続位置 | 対象への行cursorなし | next_line=6110 |
| 実rustc結果 | 未修正はexit1/E0425 | 一意置換後exit0 |

主シナリオはFinish FAIL→ReadRange→Replace→Finish PASSの4判断、実検証2回、失敗1回、repair1。比較用Readは別に1判断。候補base/path/branchと元checkout bytesは不変、差分はlarge.rsのみ。compiler成果は候補外のTempDirへ出す。モデルはfixtureなので実モデルのコーディング能力評価ではない。

- `cargo test --offline -p kamimusuhi-resident --lib engineering:: --no-fail-fast`: PASS 37/37、exit0（unit実行1.35s、compile込19.50s）。
- `cargo fmt --all -- --check`: PASS exit0（1.42s）。
- `cargo clippy --offline -p kamimusuhi-resident --all-targets --all-features -- -D warnings`: PASS exit0（14.39s）。
- workspace fixtureの初回1件はcompiler出力先 `/dev/null` のrenameがEBUSYでFAIL。候補外TempDirへ変更し13/13 PASS。製品コードの失敗と混同しない。
- 設定/使い方は `docs/engineering-agent.md`、実測は `evaluation/harness/native-engineering-20261109.json`。

コミット: `feat(engineering): 大きなソースの部分読取と修復を実装`。

### Cycle 10 の結果

実resident/HTTP fixtureを2件拡張。製品routerのbench時間は変更せず、loopback providerだけで測定した。

| 実daemon scenario | 不正action回復 | auto provider 503回復 |
|---|---:|---:|
| 実model POST | 5 | 4 |
| durable decisions/actions | 3/3 | 3/3 |
| model_failures | 2 | 60 |
| 実検証 | 1 | 1 |
| Ready | PASS | PASS |

- fenced JSONのWriteと未知field付きFinishを拒否し、その時点のaction/pending/候補に副作用0。次のRead/Write/Finishで継続した。
- 503後のbenchは実測60,170ms。probe64回でcompletion障害は解除されず、期限後の成功で回復した。model_failures=60には最初のprovider障害1回と、送信先なしのローカル失敗59回が含まれる。POST60回という意味ではない。
- `cargo test --offline -p kamimusuhi-resident --test engineering_http -- --nocapture`: PASS 4/4、exit0（64.38s）。対象clippy PASS、fmt/diff check PASS。
- 課題: 長いbench中に最初の503観測が12件の窓から落ちる。最初の原因を保持する改善をBACKLOGへ追加した。

cycle9公開commit: `82c38b2153941a38dd98fab64f6b4b7eb887b43d`（local/remote tree一致確認）。
コミット: `test(engineering): モデル応答とprovider障害からの継続を検証`。

## Cycle 11 — 中断前後で行動番号を再使用しない（着手前）

本当の目的: 結果不明の行動と次の判断を同じ番号で記録せず、再開後の成果とtraceを区別できるようにする。
前提と確かめ方: 現在の採番は完了数actions+1なので、claim後に中断してactionsが増えなければ次も同じ番号になる。decision数と結果保存数を混同せず独立cursorを持たせる。
一番安い確認方法: 保存済みpending Writeと既に書かれたbytesから再読込し、次のReadが旧sequenceを再使用することを先に再現。旧schema全件検査→cursor導出→回復→v2全件検査→一度の原子保存をfixtureで検査する。

担当: checkpoint.rsの版別意味検査はharness_audit、tests.rsの移行/番号再使用fixtureはresearch_harness、rootはengineering.rsのclaim/回復/traceを統合する。

## 2026-11-10 のコンテナ復旧

ユーザーの状況確認を受けて確認したところ、作業コピー、Rust toolchain、一時worktreeと実行ログが消失していた。GitHub branchはcycle10の `3ab5254a7c66f971f5673b9509d1699bcb1ae139` まで保存済み。cycle11 local commit `6fc741eb46566fdc2194999c13894c1035e00be1` のtree `5f579fb271fdd4dba7573b9110f8f8cc0c8a91d7` はGitHub APIで404。転送済みとは扱わない。

前セッションのtool出力には、cycle11の工程45/45 PASS、resident lib/allfeatures clippy PASS、cycle13の実daemon claim中断1/1 PASSが残る。ただし該当コード/ログは現在の作業コピーにはない。会話記録から機能を復元し、新しい版として再検証する。旧treeと同一の復元と主張しない。cycle11凍結全CIとcycle12修正前試験の最終結果はUNKNOWN。

時間は最後に確認した05:25:23 UTC（累計2.016521時間）まで引継ぐ。そこから11月10日03:59:54 UTCまでの空白は加算しない。約2時間の実作業であり、24時間継続達成ではない。

復旧の本当の目的: remoteに保存済みのnative agentを失わず、確認済みだった再開の修正を再びreview可能なコードと試験へ戻す。
前提と確かめ方: cycle10まではGitHubから取得できる。cycle11の採番/lease変更とfixtureは会話に仕様・差分・測定結果が残るため、再実装後に同じ失敗契約を実行する。
一番安い確認方法: clean cloneとpinned Rust1.92.0を用意し、cursor/leaseの対象fixtureとresident clippyを先に通して小さく保存する。
