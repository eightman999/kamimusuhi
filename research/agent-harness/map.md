# map: agent-harness

問い: 永続residentが長時間タスクを継続・再開し、外部調査と委譲の失敗から回復するために、どの設計と評価が有効か。
題: 永続エージェントハーネス
読み: えいぞく ちょうじかん けいぞく さいかい ちょうさ いじょう しっぱい ひょうか
最初の検索語: agent harness long running checkpoint resume / エージェント ハーネス 長時間 タスク 再開

## 調査の論点

- [ ] この分野の知見はどこに集まっているか（媒体）。
- [ ] 「永続residentの長時間タスクの継続と再開」はどこで失敗し、checkpointと冪等性で何を保証できるか。
- [ ] コンテキスト圧縮で目的・制約・作業記憶はどれだけ失われ、元になった研究は何か。
- [ ] tool loopと委譲失敗からの回復は、どの条件で成功率や往復数を改善するか。
- [ ] ハーネス評価で、定決的モックとSWE/長期タスクの実評価はそれぞれ何を検証できないか。
- [ ] 外部ページ・検索結果・調査手順の間で、権限とprovenanceをどう隔離し、どこに限界があるか。

## 用語

- Agent-Computer Interface — https://arxiv.org/abs/2405.15793 （原著本文の保存は未完了）
- checkpoint / idempotency key / unknown outcome — https://docs.langchain.com/oss/python/langgraph/fault-tolerance
- compaction / structured note-taking — https://www.anthropic.com/research/effective-context-engineering-for-ai-agents
- finite response sequence / dependency evidence — https://microsoft.github.io/waza/guides/dependency-recovery/
- indirect prompt injection / sandbox boundary — https://platform.claude.com/docs/en/managed-agents/secure-deployment-risks

## 人

- Justin Young — 長時間ハーネス記事の著者欄。https://www.anthropic.com/research/effective-harnesses-for-long-running-agents
- John Yang、Carlos E. Jimenezほか — SWE-agent論文の著者欄。https://arxiv.org/abs/2405.15793
- MaryCache — imozuruの公開repo・MIT帰属。https://github.com/MaryCache/imozuru
- 個別所属は独立確認していない。

## 媒体

- Anthropic実装者の記事・Claude Platform公式文書。
- LangGraph公式の継続実行・障害回復文書。
- Microsoft waza公式の決定的fixture評価。
- NeurIPS proceedings / arXivの原著。
- GitHubの公開実装（仕様・ライセンス・版を固定して読む）。

## 原文（sources/）

| 論文・資料 | ファイル | 元のURL | どこから来たか（周・動き） |
|---|---|---|---|
| 〔本人〕長時間ハーネス | web-primary-excerpts.txt:47–60、web-primary-follow.txt:5–43 | https://www.anthropic.com/research/effective-harnesses-for-long-running-agents | 英語探索と日本語探索。Web抽出本文のみ |
| 〔公式〕冪等性と障害回復 | web-primary-follow.txt:100–150（Retry boundaries and idempotency） | https://docs.langchain.com/oss/python/langgraph/fault-tolerance | 英語探索。Web抽出本文のみ |
| 〔本人〕コンテキスト管理 | web-primary-follow.txt:160–175（Compaction） | https://www.anthropic.com/research/effective-context-engineering-for-ai-agents | 英語探索。Web抽出本文のみ |
| 〔本人〕複数agentの運用 | web-primary-follow.txt:188–196（Production reliability） | https://www.anthropic.com/research/multi-agent-research-system | 英語探索。Web抽出本文のみ |
| 〔公式〕決定的な障害注入 | web-primary-excerpts.txt:434–439 | https://microsoft.github.io/waza/guides/dependency-recovery/ | 英語探索。Web抽出本文のみ |
| 〔公式〕外部入力と権限境界 | web-primary-follow.txt:400–404 | https://platform.claude.com/docs/en/managed-agents/secure-deployment-risks | 英語探索。Web抽出本文のみ |

原文はignoreされた `sources/` にのみ保存する。この表は位置とリンクのみを公開する。

`fetch.py` は7URLともコンテナの名前解決でFAILした。安全設定を緩めず、認可済みWeb取得による本文抽出をローカルへ保存した。これはHTML/PDF全文の保存成功とは区別する。抽出ファイルは公開しないので、別コンテナではURLと節名から再取得する。

## 次の手がかり

- 長時間ハーネス記事のquickstartリンクをたどったが、Web取得は制限付きURLとして失敗した。原文保存＋リンク先の取得という1周は未完了。
- 同じ検索語 `agent harness long running checkpoint resume` をRustXNGへGET/POST。両方HTTP200だが結果0件・不応答engine9。組み込み検索は一次情報を返した。空集合とのURL一致は0件だが、環境障害があるため検索品質・飽和率の比較には使わない。
- 中断により調査は未完了。全論点の原文位置や70%飽和を満たしたと判定しない。
