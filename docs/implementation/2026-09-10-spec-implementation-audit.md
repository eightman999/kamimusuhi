# 仕様対実装の監査・修正結果 — 2026-09-10

Status: **implemented and tested / W5–W7 boundary hardening**

## 基準とスコープ

監査開始時の master は `5e8817f176729f251fd309da88d932ddb88f4576`。
`spec.md`、`architecture.md`、README（日英）、docs index、W0–W7 の計画・実装結果、
`runtime-authority-learning-boundaries.md` と `organ-contracts-and-implementation-plan.md`
の authority/provenance 契約を照合した。研究ノートは非規範の提案として扱い、
W8 以降の belief graph、自律目標、self mutation、学習、分散身体を勝手に有効化していない。

## 修正した不整合

| 問題 | 実装変更 | 検証 |
|---|---|---|
| resource の `LocalOnly` が Persona dispatch には適用されず、取得記憶を外部へ送信できた | `PersonaSetting::check_privacy` を turn の dispatch 前に実施。Persona は router に登録しない。locality 未指定は `external` | CLI の privacy/locality matrix、拒否時の zero request、旧設定、URL 交換を別プロセスで検証 |
| Content-Length 未検証で、valid JSON を含む途中切断を成功扱いできた | 長さ一致・chunk/trailer 終端・UTF-8・framing を検証。8 MiB wire / 64 KiB header・trailer 上限 | 不足/余剰 body、重複・競合 framing、chunk 境界、overflow、実 socket のサイズ上限を検証 |
| provider の任意 error code/type、HTTP version/chunk text が diagnostics に混入できた | error object を共有の固定分類へ正規化。不明な code は `provider_error`。URL/header 検証も入力値を反射しない | 両 backend からの secret-bearing / malformed response を CLI、trace、DB/WAL で確認。Header Debug は値を redaction |
| typed envelope にある continuity/session と item metadata が serialization で欠落した | continuity/session、turn context、input evidence ID、全 WorkspaceItem を section + JSON record で渡す | round-trip と改行・偽見出し入り payload を検証。これは LLM の意味的安全性の証明ではない |
| physical retry ごとに logical timeout を再発行していた | retry と backoff が一つの Instant deadline を共有。直接 adapter 呼出しでも config を再検証 | 2種類の deadline/backoff regression、無効 config が接続前に拒否されることを検証 |

変更先は `kamimusuhi-resource-http`、`kamimusuhi-persona-http`、runtime の
config/CLI/scenario、HTTP fixture と回帰テスト。新しい外部依存は追加していない。
HTTP の Persona と cognitive resource は、transport と安全な error 分類だけを共有する。
別の ID 型・config namespace・trace attribution は維持している。

Persona URL の交換は旧 endpoint の `auth_env`、private CA、locality を自動継承しない。
endpoint/model を変えた場合の backend ID も更新する。
同じ URL/model 名の裏で重みが交換された場合の識別までは保証しない。

## 実測した検証

GitHub-hosted **Ubuntu 24.04 / Rust 1.92.0** で `scripts/ci-local.sh` を実行した。

```text
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
kamimusuhi-core dependency boundary
```

修正前は **266 passed / 0 failed / 0 ignored**、修正後は **293 passed / 0 failed / 0 ignored**。
これは各 harness の出力を合算した従来の集計で、環境変数テストの子プロセス再実行2件を含む。
子プロセス分を重複計上しない登録テストは **264 → 291（+27）**。
format、clippy、core 依存境界も成功。Rust を実行した場所はローカルの編集環境ではなく CI runner である。

検証したコード tree は `60cf6a9d3f7cf0d3e30f4d1540295e0b6378af6a`。
[検証 run 34377020905](https://github.com/eightman999/kamimusuhi/actions/runs/34377020905)
の artifact に revision/tree、source archive、通常 CI、復元後 CI、mutation test のログを保存した。
artifact の保持は7日であり、恒久保存を約束するものではない。

追加テストが不具合を見逃さないことも、二つの意図的な変更で確認した。

1. Content-Length 不一致判定を無効化すると、valid-JSON truncation テストが失敗した。
2. Persona privacy 判定を削除すると、CLI の privacy matrix テストが失敗した。

いずれも compile error ではなく test failure を確認し、変更を復元した後に
全 CI を再実行して成功した。恒久 CI も同じ local script を使い、任意の外部 LLM/API は呼ばない。

## 失敗時に保つもの

Persona/resource の失敗時に、IndividualId、root/head/generation、durable memory、
Library が変化せず、final-expression event が追加されない経路を検証した。
これは「DB の全 byte が不変」という意味ではない。入力の原証拠、session、
失敗の operational metadata はそれぞれの owner が保存し得る。
モデル出力は依然として canonical self-state を直接変更できず、
モデル-backed Persona の `proposals` は空である。

## 運用上の制約と未確認事項

- locality は operator の宣言であり観測・attestation ではない。`local_host` と宣言した proxy が外部へ転送しないことまで検知しない。HTTP backend は `in_process` を宣言できない。
- HTTP は blocking HTTP/1.x の限定実装。Content-Length または chunked framing を要求し、EOF-only framing、URL 内の認証/query/fragment、proxy、streaming、HTTP/2 は対象外。wire 全体8 MiB、header/trailer 各64 KiBの上限を持つ。
- socket/retry/backoff は deadline を共有するが、OS の同期 DNS resolver はこの実装では中断できない。厳密な wall-clock 上限を DNS 待機まで保証したとは主張しない。
- JSON framing は payload と構造の混同を防ぐ。LLM の prompt-injection 耐性、正答率、人格一貫性を測定したわけではない。
- scripted HTTP/TLS fixture と実プロセスの成功は **実モデルによる会話成功ではない**。llama.cpp/Ollama/LM Studio 等の実 LLM smoke test、モデル名・重み版・応答評価は未記録のままである。
- self-domain mutation、belief graph、retention/deletion closure、常時背景認知、online learning、K-Nerve、分散 authority は今回の実装対象外。

README（日英）と spec（日英）にはこの区別、Persona privacy、serialization/transport の
境界を反映した。W6/W7 の過大な検証表現は訂正し、当時のテスト件数は歴史的結果として残す。
