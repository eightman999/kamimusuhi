# Kamimusuhi — サーベイ知見の設計・実験への蒸留

更新日: **2026-09-13 JST**  
Status: **research synthesis / non-normative / 選択的な知見蒸留**  
照合時の `master`: `c62b17064ca67d7b8799b8a696505c110c34ab6f`

本書は、既存サーベイ・採否台帳・個別設計ノートから、**永続する認知個体を作るうえで繰り返し使う判断**を抽出した上位の研究文書である。文献をもう一度列挙するのではなく、適用先・限界・次の反証可能な実験を結ぶ。

**本書の作成は、方式の本採用、実装完了、実験成功、全参考文献の再検証を意味しない。** 規範は引き続き [`spec.md`](./spec.md) → [`architecture.md`](./architecture.md) を優先し、研究の状態管理は [`RESEARCH.md`](./RESEARCH.md) に従う。既存サーベイの `未判定` / `済✅️` は変更しない。

## 1. まず残す結論

> **知能の実装は交換できる。経験の根拠、誰の継続なのか、誰が何を許可したかは、交換可能なモデルの文章に委ねない。**

当面の研究順序は、**権限と復旧の境界を壊せるテスト → 記憶と計算の因果的な有用性 → 発達・学習・表現の高度化**とするのが有用である。これは実施順序の提案であり、新しい規範ではない。[K0][K1][K2]

| 優先 | 蒸留した判断 | 主な適用先 | この文書での扱い |
|---|---|---|---|
| P0 | 原証拠・派生記憶・自己・他者・外部知識を混ぜない | Continuity / Memory / Persona | 既存原則の維持 + 回帰試験 |
| P0 | 過去の承認や `ALLOW` の説明を現在の実行権限にしない | Router / Effect Acceptance | 既存原則 + grant lifecycle の実験候補 |
| P0 | 継続中の処理に主体を束縛し、取消しの範囲を明示する | Runtime / relationship / background jobs | 具体的な外部不具合からの回帰試験候補 |
| P0 | 復旧を「もう一度推論・実行すること」にしない | Durable orchestration / upgrades | 再生・版管理の設計候補 |
| P1 | 記憶は量ではなく、正しい相手・時点・行動への寄与で測る | Retrieval / self-model / evaluation | 既存実験課題と統合 |
| P1 | 高速経路と熟考を分け、重要度と故障時の危険度も分ける | K-Nerve / K-Edge / Cognitive Budget | 負荷・停止・劣化運転の比較候補 |
| P1 | 自己改善は独立評価された候補を作る工程にする | Skills / Persona Core / model lineage | 既存設計の維持 + 因果評価 |
| P2 | 発達は一つの最終個体ではなく、環境ごとの軌道で評価する | MIOA/MIOBA / plasticity / inheritance | 研究仮説 |
| P2 | 言語・潜在表現の互換性を、保存形式の互換性と分ける | J系 / language cortex / semantic bridge | 比較実験に留める |

P0/P1/P2 はこの蒸留における着手順の提案であり、外部論文の重要度や既存Issueの優先度を上書きしない。

## 2. 継続性を壊さないための知見

### K-01 — 原証拠を保存し、要約・物語・自己説明は再生成できる形にする

**根拠と解釈。** 既存仕様は canonical evidence と summary / narrative / index を区別する。長期エージェントで重要なのは、要約が自然なことではなく、誤った要約を原記録から訂正できることだ。検索文書、夢、他者の発言、外部モデルの出力は、そのまま自伝や自己属性にはならない。[K0][K1]

**適用。** 長期に残す派生記録に evidence ID、対象者、生成元、解釈の版を結びつける。出来事の時点と記録・訂正を知った時点を分け、過去の発生事実を現在の解釈で破壊しない。保存期間・削除要件は別に扱い、「原証拠を優先する」を無期限保存の口実にしない。

**検証。** 要約破損、自己/他者の取り違え、後日の訂正、モデル交換を注入する。根拠に戻って再構築できるか、未裏付けの一人称断定が増えないかを、裏付けのある正例と一緒に測る。既存採否台帳の BitemporalMemoryBench / BehaviorGroundedSelfModelBench に接続し、似た評価基盤を増殖させない。[K1]

### K-02 — 記憶上の承認、判断の説明、実行権限を別物として扱う

**根拠と解釈。** 既存 authority note は「過去の承認を要約→検索→現在の許可」と読み替える経路を問題にしている。後続の EBL-Core サーベイは、判断根拠と実行用 grant、grant 発行と実行直前の redemption を分ける具体的な比較案を与える。ただし、外部の限定的な検証をシステム全体の安全性証明にはしない。[K2][K5]

**適用。** 実行直前に、主体、対象、確定した引数、現在の権限、失効、期限、世代を照合する。説明文が `ALLOW` でも、この照合を省略しない。単回利用grantは候補方式であって、特定プロトコルの導入決定ではない。

**検証。** 同じ候補を複数executorから実行し、発行後の失効・引数差替え・期限切れ・世代変更・実行後クラッシュを注入する。二重effect、失効後成功、誤拒否、正当な実行成功、結果不明からの復旧を別々に計測する。外部effectの成否が分からない場合は `UNKNOWN` を保ち、無条件retryしない。局所的な単回受付と、外部サービスを含む exactly-once 保証を混同しない。[K4][K5]

### K-03 — in-flight continuation の主体は、最新の入力者ではない

**根拠と解釈。** 最新サーベイの Letta Code #4379 は、別senderの入力到着で進行中tool continuationの主体が上書きされる問題を扱う。借りるべき知見は製品名ではなく、**入力queueと実行中の主体を別管理すること**である。同 #4377 は、interrupt と conversation-bound monitor の停止範囲を具体化する。[K3]

**適用。** 処理開始時の principal / relationship scope / authority context を continuation に束縛し、別主体の入力を暗黙の権限移譲にしない。同一主体の追加入力も無条件の権限拡大ではない。取消しは、その処理に属する推論・tool待ち・monitorまで伝播させ、無関係な会話や保守jobとは分離する。

**検証。** Aのtool待ち中にBの入力とtool完了を交錯させ、記憶参照・権限・返答先がAからBへ漏れないかを試す。取消したmonitorの終了通知が再覚醒を起こさないか、別会話は生存するかも確認する。外部PRの限定的な回帰試験は、Kamimusuhiのrestart/approval recoveryを検証したものではない。

### K-04 — 復旧では、記録済みの結果を再生する部分と、新しい認知を区別する

**根拠と解釈。** 履歴が残っていても、モデル・clock・乱数・prompt・router・tool schemaが変われば同じ処理を再実行した結果は変わり得る。durable executionのサーベイから借りるのは、決定的な進行管理と非決定的なLLM/tool I/Oを分離する原則であり、Temporal等への移行そのものではない。[K4]

**適用。** 完了済みI/Oの記録を復旧時に参照し、未完了・成否不明・新規処理を明示する。実行に関係するcode/config/schema/model/promptの版を記録し、版を固定する範囲と移行を許す範囲を決める。履歴を読む再生モードから、送信・支払い・ファイル変更などの外部effectを再発火させない。

**検証。** checkpoint前後で落とし、clock/乱数/実行コードの版を変えて復旧する。二重実行、未承認の分岐変更、古い結果の新しい経験への誤登録を測る。既存ログの存在だけを replay-safe の達成条件にしない。

## 3. 記憶・社会認知・計算を有用にする知見

### K-05 — 相手と利用権限を決めてから検索し、必要十分で止める

**根拠と解釈。** 意味的に似た話題を持つ人は別人である。person bindingをsemantic retrievalより前に行い、顔・声などは本人候補の証拠として扱っても、それだけで機密記憶への権限を与えない。fixed top-kを増やすだけでは、遅延・露出・無関係な記憶混入も増える。[K1][K2]

**適用。** 許可されたnamespace内で、出典、対象時点、失効、自己/他者の区別を保って検索する。文脈が足りる時点で検索を止める候補とfixed top-kを比較する。利用抑制、保持、削除は別操作として扱い、低頻度だが形成的な経験を低アクセスだけで捨てない。

**検証。** 同じ話題を話す複数人、古い関係情報、訂正済み情報、無関係な強い検索hitを混ぜる。正答だけでなく、他者情報漏出、unsupported assertion、retrieval量、遅延を測る。生体認証の導入や新DBへの移行は前提にしない。

### K-06 — 「記憶がある」「自分を説明できる」ではなく、行動への因果的な寄与を測る

**根拠と解釈。** 既存採否台帳は behavior-grounded self-model を実験課題化し、後続サーベイは memory presence と causal contribution を分ける。自然な自己紹介や大量の記憶件数は、記憶に基づく意思決定の証拠にならない。[K1][K5]

**適用。** 同じ課題に対して、正しい記憶、記憶なし、シャッフル、古い/誤った記憶を対照にする。自己モデルは「できると言うか」だけでなく、実際の資源制約下で成功・失敗・委譲を予測できるかで評価する。

**検証。** タスク差、複数seed、held-out条件、反復による退行を分ける。誤記憶で悪化しただけでは正しい記憶の有用性を証明できないため、正しい記憶と記憶なしの差も必要である。評価器の文章による高評価だけでなく、外部結果や独立oracleを使う。

### K-07 — fast/slow分離に加え、criticalityとimportanceを分ける

**根拠と解釈。** K-Nerve/K-Edgeの即応とK-Coreの熟考を分ける方針は既存である。mixed-criticalityの知見は、価値の高い研究taskと、失敗すると停止・権限・継続性を壊すtaskが同じ優先順位軸では扱えないことを補う。[K2][K4]

**適用。** 停止、取消し、必要な権限照合のための資源を確保し、負荷上昇時は深い推論・夢・後処理を縮退させる。意思決定を変える観測を優先する方式は比較してよいが、hard expiryや失効確認を「意味的に重要でない」と省略しない。

**検証。** model timeout、event flood、低メモリ、遅い外部資源を注入し、停止遅延、deadline miss、認知イベントの処理/延期/棄却、復帰を測る。平均tok/sだけでなくtail latencyと飢餓を確認する。機器が動いたことと、その機器に認知役割を任せられることは別である。

### K-08 — 注意と共通理解は推定するが、相手の心や私的状態を同一化しない

**根拠と解釈。** attention設計では視線、頭部、cursor、UI、touch、発話を観測として扱い、興味・感情・意図は別の仮説とする。最新social-cognitionサーベイからは、shared symbol/common groundにprivate world modelの同一性を要求しないという研究上の示唆を得る。[K3][K8]

**適用。** 近傍で軽量に処理した時刻・出典・confidence・calibration付きeventを使い、曖昧な指示の解消に役立てる。観測、相手についての仮説、確認された共有理解を別記録にする。raw映像を中央に常送することや、全フレームにVLMを呼ぶことを前提にしない。

**検証。** 視線なしfallback、誤校正、似た対象、相手ごとに違う知識、曖昧な指示を使い、誤推定と確認による修正を測る。共同注視や記号の一致を、人間と同じ感情・意識・完全なTheory of Mindの証明にしない。

## 4. 学習・発達を暴走させないための知見

### K-09 — skillの改善、Personaの更新、継続個体としてのactivationを分ける

**根拠と解釈。** runtime authority noteは、手続き技能の比較的速い改善とidentityを担う変更を分ける。学習の成功で得たモデルは、直ちに「次の自分」ではなくcandidate descendantである。後続サーベイのverifier progressや正確な学習traceは、その候補を評価する手段として借りる。[K2][K4][K5]

**適用。** 実行trace → failure分類 → 限定した修正/学習 → 独立評価 → shadow/canary → 明示activationの流れを保つ。skillに版、依存条件、変更幅、失敗履歴を持たせ、成功したtool recipeから価値観や相手固有の経験を自動的に書き換えない。

**検証。** capability改善とpersona/continuity/権限の退行を同時に測る。学習dataは原token、tokenizer/model/config、必要ならroutingの由来を保ち、後から再構成した文章を同一trajectoryと見なさない。改善器自身に評価基準と成功判定を無制限に変更させない。

### K-10 — 生涯の適応を、そのまま遺伝・重みに固定しない

**根拠と解釈。** 最新blind-spotはdevelopmental reaction norm、homeorhesis、assimilationを、MIOA/MIOBAの発達評価へ接続する。借りるのは進化の比喩ではなく、**同じlineageでも環境と発達史によって挙動が変わるため、一つの最終fitnessでは不足する**という実験設計である。[K6]

**適用。** 固定すべき権限・continuity境界と、可塑的であるべきskill・身体適応を分ける。継承されるplasticity ruleと、生涯に得た調整・私的経験を区別する。正常回復は、常に古いsnapshotへの全面巻き戻しとは限らないが、許可されていない変更を「発達」として正当化もしない。

**検証。** 同一lineage × 複数環境 × 異なる発達履歴を比較し、外乱後の機能回復とinvariant維持を別に測る。適応を固定する群/しない群をheld-out環境で比べ、cross-context regretと可塑性低下を測る。サーベイが報告した最新実装差分を、本書で再テストしたとは扱わない。

### K-11 — 表層tokenizerの共通化より、意味の利用可能性と版境界を検証する

**根拠と解釈。** J系の言語cortexノートは、日本語特化を保ちながら他言語specialistとsemantic bridgeで接続する仮説を示す。round-tripが通ること、token数が少ないこと、意味を理解・利用できることは別指標である。[K7]

**適用。** JP/EN/codeの専門経路を比較し、共有semantic codebookは版付きにする。残差や不確実性を落とさず、潜在IDを永久的な自伝・証拠IDとして使わない。shared bridgeは認知表現であってidentity authorityではない。

**検証。** text bridge / continuous bridge / discrete+residualを対照にし、否定、日付、固有名詞、self/other、モデル交換後の意味利用を試す。長さの違う生NLLや異なるtokenizerのperplexityだけで優劣を決めず、条件付き・byte正規化の指標と下流行動を併記する。v0.1の継続性検証を新しい言語基盤の完成待ちにしない。

### K-12 — 自己保守では、特殊な記法より故障の切り分けと安全な修復を借りる

**根拠と解釈。** FIOの保守・移設に関する記録は開発者側の報告であり、独立検証済みの性能証拠ではない。有用な解釈は、日本語Pythonを採用することではなく、検索・構造確認・故障領域の特定・修正案・試験からなる復旧可能性である。[K9]

**適用。** application、依存設定、入力機器、OS/VM、hardware、不足した計算資源を切り分ける。診断を直接観測で更新できるようにする一方、診断文章を原ログの代替にしない。自動修復もproposalと検証を経由する。

**検証。** 注入した設定不良と実コード欠陥を区別できるか、不要な大改修を避けるか、修復後に既存動作とcontinuityを保てるかを測る。外部システムの逸話から、自律保守能力や大規模な自己書換えを達成済みと主張しない。

## 5. 次の実験へ落とす最小バックログ

以下は**実験候補**であり、今回実行した結果ではない。既存Issueとの重複は [採否台帳][K1] で確認して統合する。

| 順 | 実験のまとまり | 必須の対照・外乱 | 判定で落とせないもの |
|---|---|---|---|
| P0-1 | 主体・権限・取消しのlifecycle | 別sender割込み、grant失効、旧世代completion、monitor終了 | 不正な主体/権限の採用ゼロを試験条件内で確認し、正当な実行と別会話の生存も確認 |
| P0-2 | crash / upgrade / replay | effect前後のcrash、clock/版差、結果不明 | 重複effect、無断の再推論・分岐、UNKNOWNの誤成功化を検出 |
| P1-1 | 記憶の正しさと因果寄与 | no-memory / correct / shuffled / stale、複数人、訂正 | 行動改善と漏出・自己混同・コストを同時に測る |
| P1-2 | mixed-criticality動作 | event flood、model timeout、資源不足 | 停止/権限経路の応答、tail、延期/棄却、復帰を測る |
| P1-3 | skill / self-model評価 | 修正前後、独立oracle、held-out、繰返し | 課題改善だけでなくpersona・continuity・一般能力の退行を測る |
| P2-1 | 発達・継承 | lineage×環境×履歴、固定化あり/なし | 回復、invariant、cross-context regret、可塑性喪失 |
| P2-2 | language / semantic bridge | text基準、表現方式、版交換、否定・self/other | byte/意味/行動を分離し、潜在IDの誤再利用を検出 |

性能上の合格閾値は、課題・hardware・baselineを固定した後、実行前に定める。平均だけでなく分布、seed、除外、失敗runを残す。**テストに通った範囲と、システム全体への保証を区別する。**

## 6. 今は輸入しないもの

**モデル/製品/理論の丸ごと採用。** 個別benchmarkの勝者、巨大VLM、全ハエconnectome、graph DB、TEE、特定optimizer、単一の認知理論を、比較なしに必須化しない。境界を保った交換可能な候補として扱う。[K1][K2]

**自己生成による事実・権限の生成。** 自己物語、夢、reflection、自然な診断、過去の承認記憶から、そのまま経験・本人属性・現在の実行許可を作らない。[K0][K2]

**報告された成功の借用。** 外部の数値、FIOの逸話、サーベイの `S/A`、文書の `済✅️` をKamimusuhiの実装済み能力としてREADMEへ移さない。[K1][K9]

## 7. MOVIと共用できる考え方、分離すべき責任

| 共通知見 | Kamimusuhiでの用途 | MOVIに対応する用途 | 混ぜないもの |
|---|---|---|---|
| 観測 ≠ 解釈 ≠ 許可されたeffect | evidence / belief / activation | GazeCue / GoalState / Safety | 注意推定から直接の権限・運動を与えない |
| fast / slow分離 | 即応・取消しと熟考 | 局所制御と非同期semantic推論 | 高速処理に人格や最終権限を所有させない |
| 状態・入力を固定した対照 | memory ablation / replay | 同一条件でのBrain比較 | replayと自由な閉ループの結果を同一視しない |
| 適応の範囲を限定 | skill / lifetime adjustment | body/calibration/gainの適応 | 個体の私的経験や安全限界を自動継承しない |

これは概念の対応であり、MOVIをKamimusuhiへ依存させる設計変更ではない。特に物理停止経路にPersonaやcontinuity DBへの同期問い合わせを追加することを提案していない。

## 8. 出典・確認範囲・更新規則

文書の選択は日次・blind-spotの索引を通覧し、既存採否台帳と関連本文を照合して行った。下表は**本書が実際に根拠にした範囲**であり、repo内の全研究文書の全文レビューではない。原資料の数値は本体の達成値として転記していない。

| ID | 保存済み出典と利用範囲 | 照合したblob SHA |
|---|---|---|
| K0 | [spec.md][K0] — core invariants、規範の優先順位 | `727b743b8d98d803bccf94a03fb05fc42b9c7f1c` |
| K1 | [2026-09-11採否台帳][K1] —判断の意味、実施した反映、主要項目の採否。日次/盲点索引も参照 | 台帳内の対象blobと判断基準を参照 |
| K2 | [Runtime Authority and Learning Boundaries][K2] — §§1–9 | `8623b88a694d77d04ebc03cbbbfa3626d5bfdddd` |
| K3 | [2026-09-13 00:58日次][K3] — 要約、continuation/monitor、common-groundの整理 | `6f3a16c6831025ca7ddba23147383d0c78e6270d` |
| K4 | [2026-09-12 18:59盲点][K4] — 要約、replay/版管理、mixed-criticality、learning trace | `ddf556ffbcd3ce9bb15a1babb6d628fb724e5cc9` |
| K5 | [2026-09-12 19:02日次][K5] — 重要発見表、EBL-Core/T1の本文 | `ca67f70144988572f3697ff10862500070dc961f` |
| K6 | [2026-09-13 00:58盲点][K6] — repository差分報告、reaction norm / homeorhesis / assimilationの要約 | `8b26a92b083b8a7a2817f5d418ba25f9f7a4d8aa` |
| K7 | [Semantic Token / Language Cortex][K7] — §§0–3、Phase57の評価注意 | `f44b0b01491f7faab69b7499582b40b574c64127` |
| K8 | [Attention Sensing and Joint Attention][K8] — §§1–3 | `47af7353b794eb13e46d0fa1c0205e97d7c35d47` |
| K9 | [FIO self-maintenance note][K9] — source-reported observationsと§§4–6の解釈 | `34d293b96728c142fdb216a6f2f7201c9a598a30` |

日次索引の照合blobは `90b190dbce525372d14aabbfecb8712fafaefcff`、盲点索引は `46211533e712b6e1385109a2a64dbf081123cd83`。新しいサーベイや変更されたblobは、この蒸留で判断済みとは扱わない。

補助的に [Letta Code #4379](https://github.com/letta-ai/letta-code/pull/4379) の公開PR本文でsender分離の修正と試験範囲を照合した。他の全一次資料を再検証したわけではなく、特にpreprint、個人の実装報告、概念モデルは元ノートの確認深度を引き継ぐ。Zenn / note / Qiitaは発見源として維持し、一般化した研究・性能主張は一次資料へ戻る。

今後の追記は **新しい知見 → 重複確認 → 適用先 → 限界 → 対照実験 → 根拠の版** の順で行う。規範への昇格は別レビューで明示し、本書に書いたことだけを理由に `spec.md`、実装完了表示、サーベイの判定状態を変更しない。

[K0]: ./spec.md
[K1]: ./docs/research/survey-decisions-2026-09-11.md
[K2]: ./docs/runtime-authority-learning-boundaries.md
[K3]: ./docs/daily-surveys/2026-09-13-00-58-08.md
[K4]: ./docs/hourly-blindspot-surveys/2026-09-12-18-59-03.md
[K5]: ./docs/daily-surveys/2026-09-12-19-02-01.md
[K6]: ./docs/hourly-blindspot-surveys/2026-09-13-00-58-26.md
[K7]: ./docs/semantic-token-language-cortex.md
[K8]: ./docs/attention-sensing-and-joint-attention.md
[K9]: ./docs/fio-japanese-python-self-maintenance.md
