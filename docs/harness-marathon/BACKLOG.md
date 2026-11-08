# 改良候補

## 次にやること

ベースラインCIを実行して結果をJOURNALへ記録する。次にRustXNGのchecksum検証・起動・検索契約確認を済ませ、sidecarをモック試験付きで実装する。

## 優先順位

効果×確度÷コストで選ぶ。未着手を常に5件以上残し、完了項目はJOURNALへ移す。

| 優先 | 項目 | 最も安い確認 / 完了条件 |
|---|---|---|
| P0 | RustXNG fullの導入と実API契約の確認 | 公式releaseのSHA256一致、help、loopback health、JSON検索方式の観測 |
| P0 | residentの `web_search` sidecar | 明示bin/既存URL/port/settings、loopback限定、期限、故障分類、モックHTTP試験 |
| P0 | 検索のtool_catalog・allowlist・承認・judge・provenance接続 | 呼出し経路を追い、拒否・許可・外部データ扱いの決定的試験 |
| P0 | imozuruの調査とmap/log | 2つの探索方法、原文保存と1回以上の追跡、論点の証拠または飽和を記録 |
| P0 | residentへのimozuru手順導入 | SKILL互換読込案と専用tool案を比較、安全境界と出典を保持 |
| P1 | 調査資料の私設宛先・注入・過大出力の遮断 | fixtureでscheme/host/redirect/内容/サイズの境界試験 |
| P1 | 委譲失敗から直接toolへ復帰 | 失敗プロバイダの決定的シナリオで復帰率と往復数を比較 |
| P1 | checkpoint/resumeの冪等性 | 中断直前/直後を再現し、副作用の重複と継続性を計測 |
| P1 | tool loopのrepairと予算 | 不正JSON・未知tool・連続失敗の有限回復を比較 |
| P1 | コンテキスト圧縮時の制約保持 | 目的/禁止事項/未完了作業/根拠の保持率をfixtureで測る |
| P1 | self_improveの評価強化 | 自己申告成功と実終了コードの不一致を再現して拒否を確認 |
| P2 | traceと測定レポート | failure code/回数/再開状態を決定的な集計として記録 |
| P2 | specと実装の整合 | 仕様に結びつく未検証のinvariantを列挙し小さい試験へ落とす |
| P2 | flaky試験の切り分け | 既存CIで実際に揺れた試験だけを反復して原因を記録 |
