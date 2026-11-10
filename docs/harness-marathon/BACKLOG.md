# 改良候補

## 次にやること

cycle12の47件・全target clippy・fmtがPASSし、観測配列63,785→24,351bytes、残すReadRangeのJSON完全性を改善した。cycle13までを凍結した全CI実行中。次はcycle14のcontext反応型mock評価を登録して測定し、cycle15で長いprovider障害の最初の原因を永続化する。native agent本体を優先し、検索/skill統合は保留。

## 現在の最優先（2026-11-09更新）

| 優先 | 項目 | 完了条件 |
|---|---|---|
| P1 | 短い作業記憶と復元 | objective/制約/次step/検証失敗を保持して長期継続 |
| P1 | 自律loopのtraceと評価 | 決定的成功率/repair/再開重複/action数を記録 |
| P1 | claim済みactionの実process中断 | コマンド途中のkill後に副作用と未確認receiptを保持し、自動再実行しない |
| P1 | provider障害の最初の原因の保持 | 60秒bench中の繰り返しにより元の503が観測窓から消えない |
| P1 | 作業記憶の更新と長期修復 | 12件より長い失敗修復シナリオでも目的・重要な根拠・次stepを保持 |
| P1 | 不完全なprocess出力の伝播 | stdout_truncatedを外部executorが完全な成功protocolとして扱わない |
| P1 | 検証receiptと候補の変化 | Ready後の外部変更・resume後の再編集で古い成功証拠を再利用しない |
| P2 | 全CIの終了不能の切り分け | 既存PID制約とwrapper exit137を区別し、次回はtest threadsも1で観測 |

## 後続候補（検索・skill統合は保留）

効果×確度÷コストで選ぶ。未着手を常に5件以上残し、完了項目はJOURNALへ移す。

| 優先 | 項目 | 最も安い確認 / 完了条件 |
|---|---|---|
| P0 | RustXNG実エンジン到達の切り分け | 導入/health/GET/POSTは済み。0件・不応答9engineの原因をネットワーク可能な環境で確認 |
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
| P0 | 承認のdisk保存失敗時に実行しない | claim永続化失敗を注入し、副作用回数0を確認 |
| P1 | 承認IDの同一ms衝突防止 | 時計固定で2件queueし、別対象を独立に決定 |
| P1 | tool_enable後の委譲repair | enable→未着手約束→delegateを実呼出し数で比較 |
| P1 | tool schemaのadditionalProperties保持 | 辞書型/閉じたobjectの制約が提示後にも残るか |
| P0 | language race候補による副作用の重複 | barrier同期した2providerの実副作用回数と採用traceを比較 |
| P1 | process runnerの出力洪水時deadline | 改行無し/連続stdout/stderrのfixtureで期限とメモリ上限を確認 |
