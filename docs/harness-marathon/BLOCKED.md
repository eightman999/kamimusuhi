# 未解決の制約

承認や環境の不足があっても、独立して進められる候補へ移る。

| ID | 項目 | 理由 | ユーザーが行う具体的な操作 | 現在の扱い |
|---|---|---|---|---|
| B001 | 指定文書 `docs/primary-agent.md` | 起点masterに存在しない | この名称の資料が別ブランチにある場合、そのブランチへ後で追加または場所を記録する | 他の設計文書と実装から把握して続行。回答待ちではない |
| B002 | 24時間の単一セッション継続 | コンテナ/タスクの最大寿命は保証されていない | 中断された場合、実験ブランチをcloneして同じ依頼で再開する | STATEの実測時間を引継ぎ、未達を完了扱いしない |

## 解決済み

- cwdにcloneが無い: 公開repoを認可済みの経路で取得し、実験ブランチを作成した。
- Rust未導入: repo指定の1.92.0を導入した。
- imozuru未導入: 指定URLから導入しSKILL.mdを読んだ。
- `git push`のcredential無し: 既存GitHub connectorのGit Data APIで保存。新規credentialは不要。tree一致を確認し、force更新を使わずrefを同期する。

## 切り分け中

- 初回baseline CIはclippy段階でexit101だが、保持ログに診断本文が無い。元HEADの別worktreeで再実行し、ソースの既存失敗か環境要因かを確認する。現時点でユーザー操作は不要。

## 次回の環境で確認する事項

| ID | 項目 | 観測した理由 | ユーザーが行う具体的な操作 | 続行可能な作業 |
|---|---|---|---|---|
| B003 | imozuru原文の直接保存 | `fetch.py` が7URLでDNS解決失敗。安全設定は解除していない | 直接DNSと公開HTTPSが利用できる作業環境で同じURLを取得する。`IMOZURU_ALLOW_PRIVATE` は不要 | fixture試験、認可済みWeb取得の本文抽出、URL/節名の整理 |
| B004 | RustXNGの検索結果取得 | healthとJSONは200だが不応答engine9、結果0 | 公開検索エンジンへ到達できる環境で同じqueryを実行する | HTTP契約・sidecar lifecycle・schemaをモックで実装 |
| B005 | 元HEADのfull CI | 再試験はfmt/clippy/buildまで通過。workspace testで `agent_exec::executor::tests::an_attached_server_is_started_reused_and_reaped` のFAILを観測後、中断。最終終了コード無し | 実験ブランチ再開時に元HEADのこの試験を隔離実行し、環境依存か既存欠陥か調べる | 新機能の対象crate/fixture検証 |

### B005の原因確認（2026-11-09）

隔離実行で既存attached-server試験の孤児置換だけがFAIL（exit101）。このコンテナはgetpidと/procのPIDの視点が異なり、`ps -p child.id()` が空になる。起動・再利用はPASS。PID namespaceに対応する/procをmountしたコンテナ、または通常のLinux/Mac環境で同じ試験を実行する。今回executor.rsを変更してこの環境を迂回することはしない。
