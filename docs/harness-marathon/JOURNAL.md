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
