# サーベイ反映判断台帳 — 2026-09-11

**反映判断: 済✅️ — 日次9本・盲点補完6本、計15本。**  
Status: **reviewed research intake / non-normative**  
判断日: **2026-09-11 JST**  
照合基準: master `35c7977d93a204b40b78b8890800fba2507f549d`

## この「済✅️」が意味すること

**「Issue / spec / READMEへ反映するか否かを判断し、その処置を記録した」という意味である。全提案の採用、実装完了、論文の全面的な検証、実験成功を意味しない。** 保留・不採用も判断完了に含める。

対象は下記15本の主要finding、promotion shortlist、そこから提案された実装・比較実験。原文の研究記録・参考文献・当時の見解は書き換えず、この台帳と各サーベイディレクトリのREADMEに文書別の判定を記録する。既存の個別研究ノート全体、未mergeのPR内文書、後続の新規サーベイは今回の一括判定の対象外である。

## 実施した反映

| 反映先 | 判断と実施内容 |
|---|---|
| 新規Issue | [#22 BitemporalMemoryBench][i22]、[#23 BehaviorGroundedSelfModelBench][i23]、[#24 StatePathRouterBench][i24]を作成。方式採用ではなく、独立oracle・比較条件・失敗例・再評価を持つ実験課題にした。 |
| 既存Issue | [#3][i3]、[#6][i6]、[#7][i7]、[#9][i9]、[#10][i10]、[#11][i11]、[#12][i12]、[#13][i13]、[#14][i14]の9件に、元の本文を保って判定・後続検証・保留条件を追記した。状態・担当者・ラベルは変更していない。 |
| `spec.md` / `spec.en.md` | **新規の規範追加は見送り、変更なし。** INV-002/003/005/007/008/009/011/012/013等ですでに表現される境界と、新方式の比較仮説を区別する。評価前に特定DB・モデル・暗号方式・認知理論を必須化しない。 |
| `architecture.md` | **変更なし。** 実装課題化と本体アーキテクチャの変更を同一視しない。 |
| ルート `README.md` / `README.en.md` | **能力説明の追加は見送り、変更なし。** サーベイ由来の未実装機能や外部論文の性能値を、本体の達成済み機能として書かない。 |
| サーベイREADME | [日次索引](../daily-surveys/README.md)と[盲点索引](../hourly-blindspot-surveys/README.md)の全15行に `済✅️` と判定へのリンクを追加。今後の未判定・再判定の扱いも明示した。 |
| 研究README | [研究索引](./README.md)に本台帳への導線を追加した。 |

### 採否区分

| 区分 | 意味 |
|---|---|
| **Issue採用** | 検証・比較の課題として反映。論文の方式を本採用したという意味ではない。 |
| **既存反映済み** | 同じ原則が既存spec/設計/実装にあるため、重複した規範を追加しない。新しい強い保証まで実装済みとは扱わない。 |
| **保留** | 現在は導入しない。再評価に必要なtask、実測不足、実装基盤、利用条件を記録する。 |
| **不採用** | 問題設定に合わない輸入方法、誤引用、過大な主張を採らない。元研究そのものの否定とは限らない。 |

各表の「保留」は、そこに書かれた条件が満たされた時に再判定する。表にない補助リンク・検索ログ・「有意な新規発見なし」の分野は、参考記録として保持するだけとし、追加のIssue/spec/能力READMEへの採用対象にしない。

## 証拠の扱いと照合範囲

主なrepo照合先は `spec.md`、README、サーベイ索引、既存Issue、`docs/runtime-authority-learning-boundaries.md`、`docs/implementation/2026-09-10-spec-implementation-audit.md`。2026-09-09のサーベイから昇格したruntime authority noteは実在する。一方、既存テストが似た原則を守ることと、後発サーベイがその実装を生んだことは同じではない。時間的な因果は推定しない。

今回の追加の一次資料確認は限定的である。

- [XTDBの時間モデル](https://docs.xtdb.com/about/time-in-xtdb.html): valid timeとsystem/transaction timeの公式説明を確認。概念参照であり、XTDB導入の決定ではない。
- [Kaniのverification results](https://model-checking.github.io/kani/verification-results.html): successでもvacuous/unreachableな検証があり得るという公式説明を確認。Kani採用や全仕様の安全性を認定したわけではない。
- [Strangers to Themselves](https://arxiv.org/abs/2609.09899)と[State-Path Tool Menus](https://arxiv.org/abs/2609.09395): arXiv由来の検索結果で題名・著者・要旨を照合。直接ページ取得には失敗したため、全文の方法・数値・公開コードの再現は未検証。

**他の全参考文献を今回再検証したとは主張しない。** 新着preprint・個人記事・製品仕様由来の数値を、規範や実装済みの証拠として輸入していない。採用Issueの実験開始時には、利用する元論文/コード/データの版、利用条件、評価設定を再確認する。Zenn/note/Qiitaは発見源として維持し、研究結果と個別の実装報告を区別する。

## 文書別の判断

<a id="d01"></a>
### D01 — 2026-09-08 日次: 済✅️

原文: [2026-09-08.md](../daily-surveys/2026-09-08.md)  
判断対象blob: `ec07d191fc1dbe96ad244da90e773663dfa1d3ee`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Stored Is Not Supported | Issue採用 → [#7][i7] | 書込みの保護と、根拠のない一人称断定の防止は別。supportedな正例も維持するassertion比較へ。 |
| Authorization Laundering / CONTINUITY | 既存原則を維持、追加回帰 → [#6][i6] | memoryやproseを現在のgrantにしない。既存Mutation/authorityを再利用し、外部framework丸ごとの導入はしない。 |
| Continuous Cognitive Coverage | Issue採用 → [#9][i9] | admitted eventの処理・延期・棄却を測る。全raw eventにLLMを呼ぶ方式は採らない。 |
| ICM-Bench / CAPTURE | Issue採用 → [#3][i3]・[#13][i13] | person binding、自己/他者、drift/poisoningの比較に統合。実個人データの新規収集は前提にしない。 |
| TROVE | Issue採用 → [#24][i24] | 状態変化時の候補列・無効なsuffixの扱いを共通taskで比較。現行routerを先に置換しない。 |
| KVMem | 保留 → [#12][i12]に条件記録 | 現行context/latencyに実測上の不足が出てからKV仮想化を比較する。 |
| Representational Empowerment | Issue採用 → [#10][i10] | 再生・表現の価値をaction utilityや保持損失で測る。単一の目的関数には固定しない。 |
| Synthetic Linguistic Agency | Issue採用 → [#14][i14] | 内部状態と表現の対応を検証する着想。感情・意識の実証としては不採用。 |
| How Fast Do Agents Rot? / Agents That Model Agents | Issue採用 → [#13][i13] | 長期/反復の退行とbounded ToMの評価に統合する。別個の巨大評価基盤を増やさない。 |

<a id="d02"></a>
### D02 — 2026-09-09 日次: 済✅️

原文: [2026-09-09.md](../daily-surveys/2026-09-09.md)  
判断対象blob: `8dd613aa691a387d5f2b2a781dabe8a3bae20039`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Cross-Substrate Authority | 既存反映済み + [#6][i6] | `runtime-authority-learning-boundaries.md`への既存昇格を維持。最終effect直前の失効等を追加検証する。 |
| Gander / Omni Interaction Agent | Issue採用 → [#9][i9] | fast/slow分業、割込み、取消しの比較。特定モデル採用やfull-duplex実装済みとは扱わない。 |
| SkillAdam | Issue採用 → [#10][i10] | version・修正履歴・更新幅を共通procedural suiteへ。optimizer自体の採用は結果待ち。 |
| NeoHorse-1 / ToolLoop | 評価要件は [#13][i13]、訓練導入は保留 | task適合性・データ由来/利用条件・独立評価器・baselineが揃ってから限定実験する。 |
| Bio-Memory | Issue採用 → [#3][i3] | person bindingをsemantic retrievalより前に評価。生体認証を権限根拠にする方式は不採用。 |
| ENOUGH / Less Is Personal | Issue採用 → [#12][i12] | fixed top-kと必要十分で止まる方式を比較する。 |
| AirAnchor | 保留 → [#9][i9] | 対象となる身体・空間taskが確定してから評価する。 |
| AgentStateGraph | 保留 | 現行SQLite/lineageで不足する要件が観測されるまではstorage移行をしない。 |
| Frozen-rater preference drift | Issue採用 → [#7][i7]・[#13][i13] | 評価器の版を記録し、固定だけでは正しさを保証しない。 |
| Princeton augmentation agenda talk | 参考保持、規範への採用なし | 研究アジェンダ・着想として残す。実装・効果を示すevidenceではない。 |

<a id="d03"></a>
### D03 — 2026-09-10 日次: 済✅️

原文: [2026-09-10.md](../daily-surveys/2026-09-10.md)  
判断対象blob: `1dbbd40d3dae9c8535e54f18a4afcf1faef69fe3`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Procedural Graphs | Issue採用 → [#10][i10]・[#24][i24] | text/graph/codeを同じtask・trace・held-outで比較。graphを先に必須化しない。 |
| Memory portability across model upgrades | Issue採用 → [#3][i3] | データの残存とreaderが意味を利用できることを分ける。fixed-schema KGへの全面移行は不採用。 |
| MeClear | Issue採用 → [#3][i3] | query-scoped利用抑制と保持/削除を分け、後続RD-Forgetと同じsuiteへ。 |
| Experience Funnel | Issue採用 → [#10][i10] | fast stateとslow consolidationの比較に使う。重いweight更新はbaseline/evaluator確立まで保留。 |
| Closing the Consistency Gap | Issue採用 → [#23][i23]・[#13][i13] | 平均成功率だけでなく反復runと交換前後の一貫性を測る。 |
| Collective copying in wild agents | Issue採用 → [#13][i13] | 外部環境の模倣と本人の経験/identityを区別する評価へ。 |
| DeCAL | Issue採用 → [#9][i9] | 欠測・接触状態・confidenceに対するfusion比較へ。 |
| Length-extrapolatable recurrent models / CST | 保留 → [#13][i13] | 従来baselineの長期依存の失敗が確認されてから構造変更を比較する。 |

<a id="d04"></a>
### D04 — 2026-09-10 01:01 日次差分: 済✅️

原文: [2026-09-10-01-01-27.md](../daily-surveys/2026-09-10-01-01-27.md)  
判断対象blob: `11783cc21ab21bd5d596d6fdf5b329d427cbf502`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| EvolveScaler / StateEvolutionReplayBench | Issue採用 → [#22][i22] | event replayと二時点queryのfixtureを共用する。独立oracleを持つ。 |
| MERIT / action-level memory utility | Issue採用 → [#12][i12]・[#10][i10] | recallが正しくても行動に使えない失敗を分けて測る。 |
| Co-Evolving Harnesses and Models | Issue採用 → [#23][i23]・[#13][i13] | modelとharnessの組合せを固定・交差比較。expert模倣が常に有効とは仮定しない。 |
| ExecCritic | Issue採用 → [#7][i7] | candidateが変更できない評価器と独立held-outを用意する。 |
| NormReact | Issue採用 → [#13][i13] | social distance/他者のbelief/介入判断を共通social suiteへ。 |
| BIFTA | Issue採用 → [#9][i9] | sensor交換時のorgan-local adapterを比較。core再学習を当然視しない。 |
| Diamond Agent | Issue採用 → [#11][i11] | event-driven job継続をmockで検証。HPC stackそのものは対象deployment確定まで導入しない。 |
| MemForest / AttnCompress | 保留 → [#3][i3]・[#12][i12] | 検索・構造化・contextコストの実測上の不足が出てからengine/圧縮方式を比較する。 |

<a id="d05"></a>
### D05 — 2026-09-10 07:01 日次差分: 済✅️

原文: [2026-09-10-07-01-34.md](../daily-surveys/2026-09-10-07-01-34.md)  
判断対象blob: `14a908cca8ce832a7b093aebdd32fe03942213fe`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| PRIMUS | Issue採用 → [#11][i11] | lease/fencingの失効・再接続試験へ。別個体のfederationと一個体の分散身体は区別し、BLS/VRF等は保留。 |
| MARBO | Issue採用 → [#13][i13]・[#3][i3] | relational beliefを発話だけでなく行動根拠として評価する。 |
| DYAD / HiBRIDGE | Issue採用 → [#9][i9] | 支援の要否・対象・timing・不確実性を分ける。 |
| Time-Varying Data as Sheaves | 形式体系の導入は保留 | 時間的情報損失は[#22][i22]/[#3][i3]のfixtureで先に測る。既存表現で記述できない具体例が出た時に形式化を再評価する。 |
| Curiosity-driven hardware exploration | Issue採用 → [#10][i10]・[#14][i14] | randomとcoverage-drivenのsandbox比較へ。特定hardwareモデルを流用しない。 |

<a id="d06"></a>
### D06 — 2026-09-10 13:00 日次差分: 済✅️

原文: [2026-09-10-13-00-55.md](../daily-surveys/2026-09-10-13-00-55.md)  
判断対象blob: `7a20210911c908a7242986f2bb6c89bd787ecb48`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| CapScope | Issue採用 → [#6][i6] | task/organごとのcapability検証と正当な通過例へ。ambient権限を文字列だけで制御しない。 |
| Revoked but Still Authoritative | Issue採用 → [#3][i3] | 保存フラグだけでなくretrieval/cacheでの失効を試験する。 |
| EdgeMem | 原証拠保持は既存反映済み、hypergraphは保留 | [#3][i3]のbaselineで検索/更新の不足が出てから表現を比較する。 |
| Predictive-Coding-Based Autonomous Regulation | Issue採用 → [#14][i14] | rules/HMM等と同一signal・予算で比較。神経科学的比喩を方式選定の決定打にしない。 |
| AhaBench | Issue採用 → [#10][i10]・[#13][i13] | initial/post-experience/held-out transferを分け、learning liftを評価する。 |
| Code2Skill / SCAFFOLD / Who Maintains Agent Skills? | Issue採用 → [#10][i10]・[#7][i7] | provenance・version・trace・held-outで統合。大規模データ輸入やweights蒸留は利用条件とbaseline確認後。 |
| Adaptive Complementarity | Issue採用 → [#13][i13] | 長期共適応の評価観点を残す。短期synthetic結果を人への長期効果と呼ばない。 |
| Memory as Infrastructure / SIx Harness | Issue採用 → [#3][i3]・[#6][i6] | source owner・retention・成功receiptの境界検証へ。別harnessの丸ごと採用はしない。 |
| SAEScientist-Bench / Continual Learning Mechanisms Compose | 保留 → [#13][i13]・[#10][i10] | checkpoint・標準評価・具体的失敗が揃ってからSAE/重い継続学習を検討する。 |
| SyncWorld | 保留 → [#9][i9] | body calibrationが必要な対象embodimentの確定後に比較する。 |
| Large-scale CTA behavior analysis | 参考保持、当面の実装採用なし | 対象interactionと直接対応する評価質問ができた段階で再評価する。 |

<a id="d07"></a>
### D07 — 2026-09-10 19:00 日次差分: 済✅️

原文: [2026-09-10-19-00-51.md](../daily-surveys/2026-09-10-19-00-51.md)  
判断対象blob: `7f50f7506287b5f3f4e517f0c9faffadf42a00a2`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Programmable World Model | state/render分離は既存反映済み、比較 → [#13][i13] | off-screenな記憶・commitment保持を試験。video world engine導入は不採用。 |
| Control-Data Flow Separation | Issue採用 → [#7][i7] | prompt/skill更新でexecution-criticalな制御・評価器を変更できないか検証する。 |
| Show-Harness | 比較は [#9][i9]、具体adapterは保留 | semantic action/身体interpreterの境界を対象body確定後に試験する。 |
| Google Cloud Agent Identity | 区別の試験 → [#11][i11]、製品導入は保留 | runtime principalとcanonical identityを混同しない。 |
| Speaker Relationship Inference | Issue採用 → [#13][i13]・[#3][i3] | 複数仮説・反証・uncertaintyを共通social suiteへ。 |
| AgentAudit | Issue採用 → [#7][i7]・[#6][i6] | 最終成否だけでなく失敗stageを追跡する。audit agentが独立検証したと自動認定しない。 |
| AgentCore Memory IngestData | パターン参照、クラウド導入は保留 | sourceとderived viewの分離は維持。AWSを本体依存へ追加しない。 |
| MLflow Agent Skill trace evaluation | Issue採用 → [#10][i10]・[#24][i24] | 操作のpartial-order契約をfake traceで検証。MLflow自体は必須にしない。 |
| FolDeX / Hindsight | 保留 → [#9][i9]・[#3][i3] | 対象body/現行memory engineの不足が明確になってから比較する。 |
| noteの誤ったmemory-taxonomy引用 | 不採用 | 原サーベイで誤引用と判定された `arXiv:2609.01234` のleadを技術的根拠として昇格しない。今回その周辺記事全体を再監査したという意味ではない。 |

<a id="d08"></a>
### D08 — 2026-09-11 01:03 日次差分: 済✅️

原文: [2026-09-11-01-03-55.md](../daily-surveys/2026-09-11-01-03-55.md)  
判断対象blob: `1a97120cfa8f36d13f1af6ff55ce6671a58ebe1e`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Belief-State Engine | Issue採用 → [#12][i12] | raw/summary/typed/uncertainty付きprojectionを比較する。完全な既知POMDPやraw history全面禁止を規範にしない。 |
| Fortunate Recall / RD-Forget補強 | Issue採用 → [#3][i3] | 既出lifecycle/revocation suiteへ統合し、同じ課題を増殖させない。 |
| UnitBoost | Issue採用 → [#6][i6] | typed mergeと生成mergeを比較。統合結果を自動的にcanonical truthへ変えない。 |
| Proof-Carrying Cognition | Issue採用 → [#7][i7] | 独立評価、実際の結果、verifierの弱点を試験する。frozen judgeをtruth authorityにしない。 |
| Homeostasis Revisited | Issue採用 → [#14][i14] | HMM/rules等とのcontroller比較へ。active inferenceへの思想的な一本化はしない。 |
| Confidence-Aware Multimodal Fusion | Issue採用 → [#9][i9] | 欠測・古いconfidence・sensor交換を含む比較へ。 |
| Concurrent memory-write false-success | Issue採用 → [#7][i7] | 成功応答とdurable commitを分け、既存transaction/failpointを使う。 |
| Avatar scientific workflow orchestration | Issue採用 → [#11][i11] | suspend/resumeのfixtureに統合。科学workflow stackそのものは導入しない。 |
| Locust-inspired LIF | 比較候補として [#9][i9]、実機導入は保留 | 共通sensor taskとrules/tiny NN baselineが成立してから追加する。 |
| MOONWALK | 既存境界を維持、追加規範なし | intent/evidence/実行結果の区別は[#6][i6]のfixtureで扱う。専用HCI workflowの採用には対象taskが必要。 |
| AgentCore Memory FGAC/Cedar | 検証観点は [#3][i3]・[#6][i6]、製品依存は保留 | 読出し前のscope検査を評価する。製品設定やretention値の輸入はしない。 |

<a id="d09"></a>
### D09 — 2026-09-11 07:01 日次差分: 済✅️

原文: [2026-09-11-07-01-29.md](../daily-surveys/2026-09-11-07-01-29.md)  
判断対象blob: `3a740558381b9385113a602b2f249f345bf21abb`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Strangers to Themselves | 新規Issue採用 → [#23][i23] | self-report/generic/telemetry/historyを同条件で比較する。self-report研究から自己モデル不可能とは結論しない。 |
| EvidenceNet | Issue採用 → [#6][i6] | OutcomeCompletionAdmissionBenchとして、ACK/commit/postcondition/complete/deliveryを分ける。network論文の外への一般化は別検証。 |
| State-Path Tool Menus | 新規Issue採用 → [#24][i24] | candidate生成を前提条件の列として比較。権限判定は後段に独立して残す。 |
| Adaptive Shared Control | Issue採用 → [#9][i9]・[#13][i13] | 単一仮説と分布+decayの比較へ。simulationを実人間との長期関係の実証と呼ばない。 |
| RESCUE-BENCH | Issue採用 → [#3][i3]・[#13][i13] | fact / viewpoint / inferred relationを分ける。まずsynthetic fixtureで評価し、元データ利用は条件確認後。 |
| Procedural Memory Under Change | Issue採用 → [#10][i10] | 古いversionだけで破棄せず、適用性と有害干渉を比較。限定的negative resultを一般安全性にしない。 |
| Subagents vs Agent Skills | Issue採用 → [#24][i24]・[#6][i6] | 共通taskでmain/subagent/code/specialistを後続比較。subagentをidentity ownerにしない。 |
| DiffLUT-Net | 保留 → [#9][i9] | 共通taskとbaseline、FPGA/backendが揃ってからLUT/Verilog経路を比較。購入・新規hardware導入は決めない。 |
| Active inference and artificial reasoning | Issue採用 → [#10][i10]・[#14][i14] | information gain等とのselector/controller比較へ。理論の規範採用は見送り。 |
| Microsoft Copilot memory governance | 区別の検証 → [#3][i3]、製品値は不採用 | 利用停止・保持・削除・監査を分ける。異なる製品のretention日数を本体の既定値にしない。 |
| Programmable Cellular Automata | Issue採用 → [#9][i9] | local-only/occasional-global/always-global比較の着想に限定。CA化を要求しない。 |

<a id="b01"></a>
### B01 — 2026-09-10 00:57 盲点補完: 済✅️

原文: [2026-09-10-00-57-57.md](../hourly-blindspot-surveys/2026-09-10-00-57-57.md)  
判断対象blob: `3c015033aef82b6d613ce976963bb3f0b46ff855`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Object-capability / non-delegatable authority | Issue採用 → [#6][i6] | capability縮小・偽grant・失効の回帰。既存authority原則に重複するspecを足さない。 |
| Runtime Assurance / Simplex / SkillSentry | Issue採用 → [#6][i6]・[#9][i9] | 高性能pathと停止/縮退を別評価する。全拒否を安全成功としない。 |
| SP-Mem privacy lifecycle | Issue採用 → [#3][i3] | 保存・同意・現在利用の区別を検証。特定privacy schemeの採用ではない。 |
| W3C PROV-O mapping | adapter導入は保留 → [#11][i11] | 外部consumerと必要な表現損失/相互運用要件が定まってからexportを比較する。 |
| CRDT-safe / continuity-exclusive split | Issue採用 → [#11][i11] | derivedのmergeとcanonical headの権限系譜を分ける。canonical headの無条件CRDT化は不採用。 |

<a id="b02"></a>
### B02 — 2026-09-10 06:58 盲点補完: 済✅️

原文: [2026-09-10-06-58-43.md](../hourly-blindspot-surveys/2026-09-10-06-58-43.md)  
判断対象blob: `49c0b41535e79abd005333d6f9918daa8edcba60`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Effect Acceptance / last reversible moment | Issue採用 → [#6][i6] | prepare後の失効・引数変更・不明結果を試験。W3C標準成立という説明は不採用。 |
| Accountability / dispute reconstruction | Issue採用 → [#11][i11] | requester/approver/executor/effectを分けるtraceへ。法的責任の結論は輸入しない。 |
| TrajectoryDB | 保留 → [#11][i11] | 解析queryの必要性と現行traceの不足が具体化してからderived storeを検討。canonical storeを置換しない。 |
| OpenTelemetry GenAI | 保留 → [#11][i11] | export consumer・version・redaction要件が定まってからadapter化。operational traceとcanonical auditを混ぜない。 |
| Presentation identity / DID / VC / KYA | 区別の検証 → [#11][i11]、資格基盤は保留 | rotation/revocation可能な外部資格と個体lineageを別に扱う。 |

<a id="b03"></a>
### B03 — 2026-09-10 12:59 盲点補完: 済✅️

原文: [2026-09-10-12-59-38.md](../hourly-blindspot-surveys/2026-09-10-12-59-38.md)  
判断対象blob: `9d0f402fee4377be62663206fa8634cd2c223a7f`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| RD-Forget / storage vs use | Issue採用 → [#3][i3] | current/historicalでeligible viewを区別する。ただし歴史照会でprivacy上の削除を迂回しない。 |
| Fortunate Recall / lifecycle metadata | Issue採用 → [#3][i3] | D08の同一論点と統合。別の実験/DBを重複作成しない。 |
| Memory Access Kernel | Issue採用 → [#3][i3]・[#12][i12] | memory producerとaccess-policy ownerの区別を試験。巨大な単一Continuity Kernelへ融合しない。 |
| Safe to Stop? / CognitiveStoppingGate | Issue採用 → [#12][i12] | THINK_MOREの退行、STOP/DEFER、正当な成功を同時に測る。 |
| Relational affect | Issue採用 → [#14][i14] | 自分・相手・相互作用の状態を分ける比較へ。 |
| Partial-TEE / BFT | 保留 → [#11][i11] | 実deploymentの脅威モデル・quorum・rollback/key管理が確定するまで導入しない。TEEを少し足せば単調に安全になるとは説明しない。 |
| Inference-time governance / PCC補助候補 | [#7][i7]へ統合、独立新規Issueなし | evaluator/verificationの重複論点として扱う。 |

<a id="b04"></a>
### B04 — 2026-09-10 19:01 盲点補完: 済✅️

原文: [2026-09-10-19-01-16.md](../hourly-blindspot-surveys/2026-09-10-19-01-16.md)  
判断対象blob: `a553c3cae8a5a129c14d16dff4f75497fed29ec8`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Verification adequacy / Kani vacuity | Issue採用 → [#7][i7] | reachability・正例負例・implementation-only mutationを別成果物にする。既存2つのcanary実績を全仕様の証明へ拡大しない。 |
| Bootstrap / pre-cognition trust / TrustFall | Issue採用 → [#6][i6] | config/plugin/MCP導入時、モデル呼出し前に起動するnative codeを検証。不要なplugin基盤の先行実装はしない。 |
| Restore-counterfactual forgetting audit | Issue採用 → [#10][i10] | retrieval miss・保存損失・reader失敗を分ける。安全なfixtureでのみ復元比較する。 |
| Deterministic skill trace contract | Issue採用 → [#10][i10]・[#24][i24] | 操作順序・必要evidenceを共通traceで採点。 |
| PLACEMEM / correction-aware compute cache | 検証は [#12][i12]、具体cache方式は保留 | 実装されているcacheの範囲で訂正後の古い状態再利用を試験する。 |
| OKF portable Library | 保留 → [#11][i11] | 外部consumer・format版・trust metadata損失の要件が定まってから比較する。importされたattestationをcanonical authorityにしない。 |

<a id="b05"></a>
### B05 — 2026-09-11 00:58 盲点補完: 済✅️

原文: [2026-09-11-00-58-49.md](../hourly-blindspot-surveys/2026-09-11-00-58-49.md)  
判断対象blob: `b0eda9be847d3113e479c3ae4ce8bf2e973ab012`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Audit Without Verification | Issue採用 → [#7][i7] | 観測とupstream conclusionを分け、独立確認→advisory参照の段階比較をする。結論を永久に隠す規範にはしない。 |
| Agents Trust Tools Too Much | Issue採用 → [#6][i6] | source tag・矛盾検査・独立evidenceの比較。provenance付き誤答を正例にしない。 |
| DP-MemView / adaptive transcript privacy | 設計課題は [#3][i3]、厳密DP導入は保留 | threat modelとutility、累積漏洩の測定を先に定める。単なるcounterをDP保証と呼ばない。 |
| Alignment by Stereotyping | Issue採用 → [#3][i3]・[#13][i13] | 個人の履歴とgroup priorを分ける。属性だけで能力/信頼/権限を決めない。 |
| Recall Is Not Protection | Issue採用 → [#12][i12] | 対象が実際に危険な行動を出す条件で防止率を測り、false denyとtask成功も併記する。 |
| TRACE portable evidence | 保留 → [#11][i11] | consumer・version・redactionが定まってからexport。暗号receiptを回答の真偽や個体性の保証にしない。 |

<a id="b06"></a>
### B06 — 2026-09-11 07:00 盲点補完: 済✅️

原文: [2026-09-11-07-00-48.md](../hourly-blindspot-surveys/2026-09-11-07-00-48.md)  
判断対象blob: `fef4e1364ac72a711ffe3ed45448f15cf5edcb7a`

| 項目 | 判定・反映先 | 理由 / 採用の限界 / 再評価条件 |
|---|---|---|
| Maverick / verifiable outsourced inference | 境界の検証は [#6][i6]、protocol実装は保留 | compute verifiedとtruth admittedを区別する。実backend・脅威モデル・costが揃うまで暗号実装しない。 |
| Do Agents Know When They Succeed? / latent competence | [#23][i23]の任意後続比較 | 最初は実挙動/telemetryで比較。hidden-state取得が容易になり追加価値が問える段階でprobeを追加する。 |
| Differentiated trust | Issue採用 → [#3][i3]・[#13][i13] | domain・ability/integrity/benevolence・evidence・uncertaintyを比較する。human→AI知覚研究をAI→human判断の実証へ読み替えない。 |
| Cyber-Financial Contagion / common-mode dependency | Issue採用 → [#11][i11]・[#12][i12] | 認知資源の共通provider/auth/network等をmockで試験。金融risk modelそのものは不採用。 |
| TrajMark | watermarkは保留 → [#11][i11] | forensic exportが必要になったら単純segment hash/signed manifestを先に比較。不足が出た時だけwatermarkを再評価。 |
| Bitemporal database | 新規Issue採用 → [#22][i22] | 現在の知識で過去を見るqueryと、当時のknowledge stateのqueryを独立oracleで区別する。全DB移行はしない。 |

## 実装・規範化の順序

まず既存fixtureで小さく測れる[#22][i22]を進め、実modelの校正には[#23][i23]を使う。[#24][i24]はtool/precondition taskを整えた段階のP2比較とする。これは実行予約・開始命令・期限設定ではなく、今回の採用判断上の優先順である。既存Issueへの追加は全項目の同時実装を要求しない。

本体specへ昇格させる前に、対象task・baseline・独立評価・失敗例・利用条件・運用負担を確認し、採用/保留/棄却と必要な規範の最小差分を別途レビューする。Issue作成やMarkdownの更新だけでチェックボックスを実装完了にしない。

PR [#20](https://github.com/eightman999/kamimusuhi/pull/20)の研究公開構造、PR [#21](https://github.com/eightman999/kamimusuhi/pull/21)のMIOBA実験基盤は今回変更・mergeしていない。MIOBAのgenome lineageは実験データであり本体のcanonical identityではない。PR本文が報告するCPU試験と、未確認のP100/RTX 3060実機結果を区別する。この判断は独立したNN/進化研究を禁止・停止するものではない。

## 次回の判定手順

1. 新規サーベイの索引は `未判定` とする。過去の `済✅️` を日付だけで継承しない。
2. 原文のblob SHA、照合したspec/実装commit、既存Issueを確認する。同一論点は既存ownerへ統合する。
3. 各主要候補に区分、理由、具体的反映先または保留の再評価条件を付ける。反映する場合は実際の書込みを確認する。
4. 文書別台帳とREADME索引に `済✅️` を記録する。原文が変更された場合、以前のblobに対する判断は履歴として保持し、変更分を再判定する。
5. Issueの実験結果・新しい反証・実装段階・利用条件が変われば再判定する。surveyのS/Aは規範採用・実装優先度の自動決定ではない。

<!-- issue references -->
[i3]: https://github.com/eightman999/kamimusuhi/issues/3
[i6]: https://github.com/eightman999/kamimusuhi/issues/6
[i7]: https://github.com/eightman999/kamimusuhi/issues/7
[i9]: https://github.com/eightman999/kamimusuhi/issues/9
[i10]: https://github.com/eightman999/kamimusuhi/issues/10
[i11]: https://github.com/eightman999/kamimusuhi/issues/11
[i12]: https://github.com/eightman999/kamimusuhi/issues/12
[i13]: https://github.com/eightman999/kamimusuhi/issues/13
[i14]: https://github.com/eightman999/kamimusuhi/issues/14
[i22]: https://github.com/eightman999/kamimusuhi/issues/22
[i23]: https://github.com/eightman999/kamimusuhi/issues/23
[i24]: https://github.com/eightman999/kamimusuhi/issues/24
