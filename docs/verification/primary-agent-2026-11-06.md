# 主推論の直接調査と委譲失敗からの引継ぎ（2026-11-06）

## 原因と修正

主推論には既に複数roundのtool loopがあったが、未着手の約束をtask_delegateへ誘導していた。
元の失敗タスク` t1a1af5d3b43 `はopencodeのAPIキー期限切れでsummaryが空だった。
また、指定された`/home/eightman/kamimusuhi`はPi上に存在せず、既存filesystem MCPの許可範囲にも入っていなかった。

- 日常的なファイル・repo・Web調査は主推論自身が提供ツールを使って完了する指示へ変更。
- 結果依存の複数round操作と、委譲失敗後の直接ツールへの有限回repairを追加。
- 登録repoの`git files`/`read_file`を追加。未追跡を含めて調査でき、秘密path・symlink・binary等を拒否。
- 実行時の明確な認証失敗を15分間のexecutor cooldownへ反映。時刻・executor名だけを保存し再起動後も維持。
- モデル一覧取得の成功ではcooldownを解除しない。通常のlogin関連テスト失敗は区別する。
- 既に待機している後続jobも実行直前に再確認し、slotと予約を解放する。

既存allowlist、approval、judge、料金境界を維持。APIキー更新、追加executor起動、
個体再初期化、非同期レビューからの無条件自動再実行は行っていない。
主推論の動作は[主推論による直接ツール実行](../primary-agent.md)を参照。

## 検証

最終ソースの公式`RUST_TEST_THREADS=1 CARGO_NET_OFFLINE=true bash scripts/ci-local.sh`が成功。
**970 passed / 0 failed / 5 ignored**。書式、全target・全featureのClippy、core依存境界も成功。

追加回帰は以下を含む。

- 主推論が約束だけの返答から直接repo調査へ進むこと。
- failed task_statusの後、委譲失敗を最終結果にせず直接調査へ戻ること。
- 未追跡docsの列挙、行番号・ページ・出力上限、秘密・symlink・binary・oversize拒否。
- 認証cooldownがモデル一覧refreshと再起動を跨いで有効で、診断本文を保存しないこと。
- 認証切れ先行jobの後、既に待機していたjobがexecutorを実行せずFailedとなり、slot/running/予約を解放すること。

## 配備

ベースはmaster `352a6e8a4c7feaea84229a2f6b54c3d54fc41f5d`、今回の未コミット変更を重ねてビルド。
変更ファイルのSHA-256 manifestを各ノードの配備記録へ保存し、Piのビルド元と一致確認。
元のPi repoの編集は上書きせず、独立した`/home/eightman/src/kamimusuhi-primary-agent-20261106`でビルド。
resident/runtimeをバックアップ後にatomic置換し、service_control経由で再起動を観測。

- Mac：最終版の起動番号 **46 → 47**。
- Pi：**61 → 62**。authoritative_state=true、pending_tasks=0を最終確認。
- Pi resident/AgentHub workerは両方active。実行中タスクのないことを確認してworkerを一時停止・再開した。
- 設定・認証値・個体SQLiteは置換していない。コミット・pushは行っていない。

## 実対話の結果

`POST /v1/kamimusuhi/talk`へ、実ファイル調査を依頼した。
合格条件は非空回答、実ファイル名一致、成功したGit呼び出し証跡、外部再委譲なし。
引継ぎケースでは元失敗タスクへの成功したtask_status呼び出しも必須とした。

| ノード | 通常のrepo調査 | 元失敗タスクからの引継ぎ |
|---|---|---|
| Mac | git 6回成功、4文書の名前・先頭見出し・検証内容を報告 | task_status + git 6回成功、4文書の見出しを報告 |
| Pi / Mio | git 4回成功、2文書の名前・先頭見出し・検証内容を報告 | task_status + git 4回成功、2文書の見出し・行数を報告 |

Piの登録repoは`/home/eightman/src/kamimusuhi-git`。以下の2文書の一覧と見出しは
ツールとは独立してfilesystemから読み、Mioの回答との一致を確認した。

- `docs/verification/mac-web-self-improvement-2026-11-06.md`
- `docs/verification/pi-agenthub-access-2026-11-06.md`

実際の主推論routeはmistral tier / deepseek-v4-1-flash-1017（既存subscription）。
全4ケースでtask_delegate/task_continueは0回。期限切れexecutorへの再実行はしていない。
元のFailed状態を成功へ書き換えず、引き継いだ調査の対話・tool evidenceを別に記録した。
Macの実対話は主推論ロジックが同一の途中配備版で実施し、その後cooldownの誤判定とqueue処理を修正した最終版へ更新。
Piの2ケースは最終版の再起動後に実施した。

## 証跡と範囲

各ノードの私有領域`current_state/deployments/`に以下を保存。

- `primary-agent-2026-11-06.json`：配備元manifest・binary hash・backup・再起動観測。
- `primary-agent-2026-11-06-live.json`：通常依頼の実回答・tool evidence。
- `primary-agent-2026-11-06-recovery.json`：失敗タスクを照会して引き継いだ実回答・tool evidence。

Mioは本人の対話ターン内で直接調査・引継ぎを実行できることを確認した。
任意の将来依頼が必ず成功する保証や、非同期レビュー通知だけでの自動再実行を検証したものではない。
期限切れのopencode認証自体は未修復。失敗理由の捏造や成功扱いへの変更はしていない。
