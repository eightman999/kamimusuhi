# API待機中自律思考の検証（2026-11-08 JST）

本書は実装時点の記録。承認後のPi/Mac本番反映と実API検証は
[本番検証記録](api-cognition-production-2026-11-08.md)を参照。

## 結果

会話中のAPI判断に加え、待機中のAPI内省をローカル実装・検証した。
本番のサービス、設定、個体DBは変更していない。

- residentは明示したsubjectだけを巡回し、foreground対話中は起動を見送る。
- runtimeの入力なし`cogitate`が、当該subjectの未処理の実会話だけを内省する。
- cursor、試行回数、日次予算をatomic保存し、排他lockで重複を防ぐ。
- 同じsourceや内省自身の出力では再起動しない。中断・intake後の失敗は不確定として保持し、
  同じsourceを自動再送しない。送信前のAPI失敗だけに有限のretry/backoffを適用する。
- 内省APIも会話と同じTLS/auth/route pin/返却モデル検証を使う。
- subject別のsource・metrics・pending draftを分離し、別人物へのmemory draftを拒否する。
- 自己改善のreplayをtoolsなし・identity-boundにし、過去の依頼の外部操作再実行を防ぐ。

設定・動作条件は[待機中自律思考](../idle-cognition.md)を参照。
通常は無会話60秒後、最短10分間隔、個体全体で1日24cycleまで。
cycle上限はschema repair/replayを含む個別API要求数とは異なり、各要求はresident CostGuardを通る。

## 検証

`cargo test -p kamimusuhi-runtime -p kamimusuhi-persona-http -p kamimusuhi-resident -p kamimusuhi-core -p kamimusuhi-store-sqlite --offline`

**948 PASS / 0 FAIL / 6 ignored**、exit 0。
ignoredは既存のopt-in/子scenario用で、PASSには含めない。

- idle engine unit: 6 PASS。未処理source、idle、最小間隔、retry/backoff、日次予算、時計rollback、
  再起動、排他、checkpoint破損/別個体/symlink、intake後API失敗の非再送を確認。
- idle CLI + loopback reflector: 5 PASS。実processからAPI内省が走り、Reflection evidenceのみが増え、
  UserUtterance/AgentUtterance/session/turnを捏造しない。再processは再送せず、別subjectも混ざらない。
- resident cognition unit: 7 PASS。明示設定、foreground skip、envのみのidentity/token引継ぎ、期限、
  安全なstatus、subject間の公平な巡回を確認。resident全回帰はlib241＋delegation4 PASS。
- reflection/replay: C0 unit14、既存C0 lane11、HTTP reflector追加2 PASS。
  他subjectの押出し防止、primary/reflector privacy、pin拒否、tool serverアクセス0、
  保存後replay503のReflectionAfterIntake分類を含む。
- 先に実装した会話中API判断のproduction child fixture6件も今回の全回帰に含む。
- 影響5crateの`cargo fmt --check`: PASS。
- 影響5crateの`cargo clippy --all-targets --offline -- -D warnings`: PASS。
- `git diff --check`: PASS。
- Mac native release build（resident/runtime）: PASS。

通常shell fixtureは並列負荷を考慮し期限2秒→10秒。timeout検証用30msは維持した。
追加fixture以外の無関係な既存変更は戻していない。

## Mac配備候補

| Binary | SHA256 |
| --- | --- |
| `target/release/kamimusuhi` | `bdbc38bd3ad6bb38ceb4de48a5b587aa60e5f244e0db3b235ce732a4d5b0e839` |
| `target/release/kamimusuhi-runtime` | `3500444571d31b9daee91a4d9535b14e00b2b14602a41ebdb1ccccf4ce48fcc0` |

Pi向けnative build、本番反映、実APIの意味品質、実Slack往復、長期間の連続稼働は未検証。
実モデルへの要求、外部発信、資格情報変更、commit/pushは行っていない。
補助的な外部批評は自動承認レビューで内部構成の外部送信が拒否され、批評結果は得ていない。
独立レビューはローカルread-onlyで行い、上記の副作用・失敗段階の問題を修正した。

## 承認後の本番反映

1. Mac/Piのresident/runtime 2binとresident設定を退避。Piは専用build領域でnative buildする。
2. 進行中turnを確認し、必要な窓口だけ一時停止。2binを配置し、resident設定には既存内容を保って
   `cognition`のsubject allowlistを追加する（既定操作者の`eightman`を想定）。
3. 個体DB・履歴・資格情報は置換/初期化しない。FI3のモデル/判断/内省実装・policy差分をレビューし、
   対応するexact review tokenで明示承認する。起動/fallbackへ自動承認を追加しない。
4. 会話中の判断→生成→評価と、専用smoke subjectの待機中内省を確認する。
   既存の会話を試験用に改変せず、同じsourceの再送0・不要なUserUtterance0・worker回収を確認する。
5. 窓口を復帰し、`/status`のcognitionと利用監視を確認。問題時は2binと今回追加設定だけを復旧する。
   個体DBや後続会話を過去へ巻き戻さない。
