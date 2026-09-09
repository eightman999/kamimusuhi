# 旧AI / MCPプロジェクト蒸留ノート（2026-09-09）

## 目的

`AINanikaAIChan`、`mcp-local_lm-cli`、`mcp-codex-cli`、`mcp-lmstudio` など、Kamimusuhi以前に作ったAI人格・ローカルLLM・MCP接続実験から、現在も価値のある設計知識だけを抽出する。

これは旧実装をKamimusuhiへ移植する計画ではない。旧repoを退役させても、同じ設計課題を再発明・再発生させないための lineage / lessons-learned 記録である。

## 1. AINanikaAIChan から残すもの

### 1.1 人格と推論資源を分ける

旧構成では「キャラクター設定」「ChatGPT / Claude / Gemini」「SHIORI」「デスクトップ表示」が一つの製品境界に寄りやすかった。

残すべき原則は逆である。

- 個体の continuity / persona / relationship state は provider から独立させる。
- モデル、API、ローカル推論器は交換可能な cognitive resource とする。
- 表示器（伺か、デスクトップマスコット、音声UI等）は embodiment であり identity ではない。
- provider変更、モデル変更、表示器変更、プロセス再起動で IndividualId が変わってはならない。

これは現在の Kamimusuhi の continuity head、Persona Core、resource slot、embodiment 分離を支持する歴史的根拠として扱う。

### 1.2 長時間常駐では「返答品質」より lifecycle が先に壊れる

旧Issue群では、API応答だけでなく次の問題が実際の主要課題になっていた。

- SHIORI / 子プロセス通信のループ・タイムアウト
- デスクトップ切替やwindow lifecycle
- 設定と再起動後状態の復元
- 複数ゴースト / 複数プロセス分離
- メモリ・CPU・長時間動作

したがって persistent AI の評価では、単発会話品質だけでなく以下を独立した試験軸にする。

1. restart continuity
2. hung / crashed resource isolation
3. duplicate delivery / retry semantics
4. bounded memory and background activity
5. embodiment disconnect / reconnect
6. state restoration without reissuing identity

### 1.3 UI / baseware と cognition の責務を混ぜない

MacUkagakaで追っていた shell / surface / animation / SHIORI / SakuraScript 互換性は、現在は主に `Ourin` 系の責務である。

Kamimusuhiへは描画・伺か互換コードを持ち込まない。Kamimusuhiが受け取るのは typed sensory/event input と embodimentへのtyped action/utteranceであり、baseware固有仕様はadapter境界の外側に置く。

## 2. 旧MCP群から残すもの

### 2.1 最小resource contract

`mcp-lmstudio` の最小構成には、今でも有効な3種類の操作があった。

- discovery: 利用可能なモデル・能力の列挙
- invoke: prompt / taskをresourceへ渡す
- health: endpointが現在利用可能か確認する

ただし、これらをprovider固有MCP toolとして増殖させるのではなく、Kamimusuhiでは provider-neutral resource descriptor / router / adapter に畳み込む。

### 2.2 OpenAI-compatible endpoint は有用なadapter境界

LM Studio等を `baseURL + model` でOpenAI互換クライアントに接続する発想は維持する価値がある。

保持するもの:

- endpointとdefault modelをruntime config / environment側へ置く
- adapterの外へprovider SDK型を漏らさない
- local / remoteを同じslot contractで扱う
- provider replacementをidentity migrationにしない

保持しないもの:

- providerごとに別MCP server repoを作る
- provider名をcanonical memoryへ書く
- adapter内部の暗黙auto-selectionを最終authorityにする
- API keyそのものを永続設定へ格納する

### 2.3 capability-based routing

旧MCP実験には general / code / vision といったモデル分類とfile-type別選択があった。この発想自体は正しいが、名前ベースのハードコードではなくresource descriptorへ一般化する。

候補属性の例:

- locality
- modality
- context capacity
- latency
- cost
- quality
- health
- privacy / egress eligibility

routerはhard constraintを先に適用し、候補がない場合は勝手に制約を緩めず拒否する。

### 2.4 「search」と「model knowledge」を混同しない

旧 `mcp-local_lm-cli` はローカルLLM回答を `search` として露出していた。これは現行設計では採用しない。

- retrieval/search と generation は別resource / evidence kindにする。
- モデル内部知識による回答を外部検索結果として扱わない。
- Library、web retrieval、model inferenceはworkspace上でprovenanceを保持する。

この区別は hallucination対策だけでなく、どの情報がcanonical mutationのevidenceになり得るかを判定するために必要である。

### 2.5 raw file pathをresourceへ無制限に渡さない

旧MCPの `analyzeFile(filePath)` は便利だったが、persistent agentの基盤としてはauthority / privacy境界が弱い。

Kamimusuhiでは、ファイルアクセスは次の順序を要求する。

1. runtimeがaccess scopeを決める
2. artifact / chunkへimportまたは明示参照する
3. provenanceとprivacy classを付ける
4. routerがegress可否を判定する
5. resourceには必要最小限のcontentだけ渡す

## 3. 明示的に捨てる旧設計

以下は lineage として記録するが、実装を継承しない。

- 人格をsystem prompt一枚に閉じ込める設計
- API/providerを人格の永続性と結合する設計
- providerごとのMCP wrapper repo増殖
- credentialsを通常設定ファイルへ保存する案
- model knowledgeを「検索」と呼ぶAPI
- 自動model selectionの理由を記録しない仕組み
- request/result bodyをresource-call監査ログへ丸ごと保存する仕組み
- UI/basewareの状態を個体のcanonical identityとして扱うこと

## 4. 現行Kamimusuhiへの対応

2026-09-09時点で、蒸留した原則のかなりの部分はすでに実装・設計へ入っている。

- W0–W5: continuity、evidence、memory、Library、resource attribution、restart
- W5: logical callとphysical attemptの分離、secret非保存、failure classification
- W6: resource router、locality/privacy hard constraint、health/cost/quality/latency、TLS transport

したがって旧MCPコードの直接移植は不要である。今後は不足しているtest / capability表現だけを現行設計上で追加する。

## 5. 回帰試験として残すべき項目

旧プロジェクトの経験を、次の回帰試験へ変換する。

1. provider A → B交換後もIndividualId / continuity headが同一
2. LocalOnly要求にlocal resourceが無ければ外部送信ゼロで拒否
3. vision要求をtext-only resourceへ誤routingしない
4. resource timeout / crashがcanonical stateを破壊しない
5. health変化でroutingが変わっても、その理由がtraceに残る
6. embodimentの再起動・交換でidentityが変わらない
7. Library/search/model inferenceのprovenanceが混ざらない
8. file artifactのprivacy classに反するegressを拒否する
9. credential / request body / response bodyがcanonical DBやoperational traceへ残らない
10. retryしてもlogical resource callが重複記録されない

## 6. 旧repo退役方針

- `AINanikaAIChan`: 歴史的prototype。新規機能開発を終了し、未完ロードマップは閉じる。
- `mcp-local_lm-cli`: 外部repo由来fork + 実験。新規開発終了。
- `mcp-codex-cli`: provider-specific wrapper実験。新規開発終了。
- `mcp-lmstudio`: 最小OpenAI-compatible adapter実験。設計原則を本書へ蒸留したため新規開発終了。

旧repoは、将来参照する場合も「実装の正典」ではなく historical evidence としてのみ扱う。
