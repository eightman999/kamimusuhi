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
| `bash scripts/ci-local.sh` | 実行待ち | 変更前ベースライン |

コミット: この節を導入する `chore(harness): 長時間改良の再開記録を初期化`。
