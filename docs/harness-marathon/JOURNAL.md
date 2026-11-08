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
