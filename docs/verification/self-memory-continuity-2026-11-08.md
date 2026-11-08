# 自己記憶と継続性の検証（2026-11-08 JST）

## 結果

自己モデルの採用・投影・訂正とコンテキスト組み立てを改善し、5本の継続性評価を実装した。
仕様・保証範囲は[自己記憶の継続性](../implementation/self-memory-continuity.md)、
モデル変更前後の比較形式は[比較評価](../implementation/continuity-evaluation.md)を参照。

本番個体・サービス・資格情報は変更せず、commit/pushは行っていない。
実LLM要求・追加課金・外部発信も行っていない。既存の未コミット変更を保全した。

## 実行した検証

1. `CARGO_INCREMENTAL=0 cargo test -p kamimusuhi-core -p kamimusuhi-store-sqlite -p kamimusuhi-runtime -p kamimusuhi-persona-http -p kamimusuhi-resident --offline --no-fail-fast`
   - **992 PASS / 0 FAIL / 6 ignored**、exit 0。doctestまで完了。
   - ignoredは既存のopt-in/独立process scenarioで、PASSへ算入していない。
2. 継続性コンテキストだけでも空扱いせず、JSON内の偽見出しが自己状態へ昇格しないHTTP境界を追加。
   `CARGO_INCREMENTAL=0 cargo test -p kamimusuhi-core -p kamimusuhi-persona-http --offline`
   - **210 PASS / 0 FAIL / 0 ignored**（core148、HTTP62）。上記との重複を含む。
3. 影響5crateの `cargo clippy --all-targets --offline -- -D warnings`: PASS。
4. 変更crateの `cargo fmt --check` と `git diff --check`: PASS。
5. 実CLIで同梱paired fixtureを採点しJSON出力を再読込。
   - exit **3**、`capture_kind: fixture`、候補/比較とも **unverified**。
   - 独立人間評定がないため、機械検査が通っても人格継続性PASSにしない。

## 新規43評価

| 対象 | 件数 | 内容 |
|---|---:|---|
| core `self_update_guard` | 9 | aggregate欠落、同値重複、容量/項目/根拠/key上限、既存項目保全、根拠差替え |
| store `c0_integrity` | 8 | 妥当JSONの投影欠落、不正JSON、proposal破損、着地失敗rollback、再open/branch、競合更新、採用前訂正 |
| store `evidence_atomic` | 4 | 発話＋全根拠linkの原子保存、途中失敗、参照境界、readback、再open/冪等性 |
| runtime `continuity_scenarios` | 8 | 下記の実対話/保存故障・再起動シナリオ |
| runtime `continuity_eval` | 13 | paired条件、期待/禁則/出所/約束、欠損・重複、評定不足、CLI拒否・終了コード |
| HTTP注入境界 | 1 | context単独の注入と偽自己状態見出しの隔離 |

runtimeの8評価は、訂正の直接/派生根拠・既存echo・作業メモの次turn除外、
履歴だけを根拠に繰り返した3turnのecho、独立CLI/新sessionでの約束の継承、
教訓を落とす更新の拒否と保全、配信後のDB保存失敗の再起動後検出、
lineage失敗時の発話rollback、legacy予算超過の明示、監視先dangling symlink拒否を確認する。
fixture providerは実際に渡した入力・自己・履歴・想起・メモを消費する。

既存の判断コンテキスト比較とCLI evidence件数は、新しいcontext欄/完了証跡へ合わせて更新し、
CLIでは2turnに対してexactly 2件の`memory_writeback_verified`を独立SQLで検査した。

## 制約と配備

- fixtureは機構と故障時挙動の検証であり、実モデルの意味理解・人格・文体維持の実証ではない。
- 5本目の評価器は実装・fixture検証済みだが、実モデル2条件の出力と独立人間評定は未取得。
- 自由文の意味的重複・矛盾・真偽、現実の約束の履行は機械検査だけでは保証しない。
- 以前の出所未連結echoや、導入前の書戻障害は遡って分類しない。未完了turnを自動再実行しない。
- 文脈に提示した履歴/記憶から保守的に依存を張るため、実際に使わなかった情報の訂正でも発話が除外されうる。
- 本番反映は未実施。新しいcontext/書戻実装はFI3差分となり、対象個体のreviewとexact tokenによるacceptが必要。

初回sandbox実行ではloopback bindが拒否されたため、ローカルfixture用権限で再実行した。
途中で容量不足になり、再生成可能な`target/debug/incremental`（約19GiB）だけを整理し、
incrementalを無効にして上記一括検証を完走した。ソース・研究データ・個体DB・release成果物は削除していない。
