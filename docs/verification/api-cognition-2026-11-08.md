# APIによる会話側推論の検証（2026-11-08 JST）

本書は実装時点の記録。承認後のPi/Mac本番反映と実API検証は
[本番検証記録](api-cognition-production-2026-11-08.md)を参照。

## 結果と対象

ローカル実装と検証を完了。本番のサービス・設定・個体DBには反映していない。
対象は会話側K-COREの判断backendで、数値KCore・GPU専用機・待機中schedulerの変更ではない。

`decision: auto`（未指定時）は、Jev設定ありなら従来のTypeSafe、Jevなし・API Personaありなら
primaryと同じAPIへ発話前判断と生成後評価を接続する。
API先に変えてもホストの推論閉ループ、個体・履歴・権限の所有を保つ。
設定と契約は[API推論](../api-cognition.md)を参照。

## 実行した検証

- `cargo test -p kamimusuhi-runtime -p kamimusuhi-persona-http -p kamimusuhi-resident --offline`:
  **713 PASS / 0 FAIL / 6 ignored**、exit 0。
  ignoredは既存のopt-inまたは子scenarioであり、PASSには含めない。
- 新規production CLI/child worker API loop: 6 PASS。
  JevキーなしAutoの判断→生成→評価、品質不足の1回repair、未知候補・不正JSONの未配信、
  External/LocalNetwork判断器のLocalOnly開始前拒否を確認。
- 判断unit: 既存Jev15件と新API10件、25 PASS。
- structured transport追加2件: PASS。
  会話用短文上限・stop・logit bias・複数候補数を判断要求へ継承せず、独立4096トークン上限と
  JSON modeを明示すること、宣言外返却モデルの拒否を確認。
- `cargo fmt --check`（上記3crate）: PASS。
- `cargo clippy`（上記3crate、`--all-targets --offline -- -D warnings`）: PASS。
- `git diff --check`: PASS。
- Mac native `cargo build --release -p kamimusuhi-runtime -p kamimusuhi-resident --offline`: PASS。

初回全回帰では旧IPCの`config: null`を前提にしたhard-timeout fixtureが1件失敗した。
fixtureだけを新しい明示`rule_based`宣言へ更新し、kill/reap/slot再利用を含む全回帰で成功した。
既存の文章生成fixtureは判断器をRuleBasedと明示し、新機能は別のproduction経路fixtureで検証した。

## ローカル配備候補

| Mac release binary | SHA256 |
| --- | --- |
| `target/release/kamimusuhi` | `bd69274127a25e2d3e852f9d278fce3d51825791a3586fd460075ea3d6d8068d` |
| `target/release/kamimusuhi-runtime` | `2b5e198f837b35839fbdb2ebd855fc2bcc85f7f4955cfb3002d8bd8b19f83744` |

Pi向けnative build・本番配備・実APIの意味品質・実Slack往復は未実施。
資格情報の表示・変更、実モデルAPI呼出し、commit/pushは行っていない。
補助的な外部批評は自動承認レビューで内部構成の外部送信が拒否され、批評結果は得ていない。
独立レビューはローカルread-onlyで実施し、LAN localityと判断用生成制約の指摘を修正・検証した。

## 承認後に行う本番反映

1. Mac/Piそれぞれのresident/runtime 2binを退避。Slack・consumerの既存構成と資格情報、
   個体DB・履歴は保全する。Piは専用build領域で同じソースをnative buildする。
2. 二重処理を防ぐため進行中turnを確認し、必要な窓口だけを一時停止して2binを置換する。
3. 既存設定の`decision`未指定はAutoになる。Jev未設定でもAPI判断は有効になる。
   FI3 statusで判断役割・prompt実装の差分をレビューし、対応するexact review tokenで承認する。
   起動時やfallback時の自動承認は追加しない。
4. 専用smoke subjectで判断→生成→評価とモデル・identity・worker回収を確認し、窓口を復帰。
   Slackへの新規投稿やAPIの追加課金が必要なら、その操作の承認範囲内だけで実施する。
5. 問題時は2binを退避版へ戻す。正典DBや後続会話を過去のbackupへ戻さない。
