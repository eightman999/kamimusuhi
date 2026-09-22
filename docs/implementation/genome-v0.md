# Genome v0

Memory とは独立した、長期的に安定する「遺伝情報」の層。人格文章ではなく、行動・認知・学習傾向を決める **0.0〜1.0 の数値パラメータ群** として扱う。

## 境界

- Genome は会話履歴・具体的記憶・persona seed を一切持たない。持てるのは 4 群 20 gene と識別メタデータだけで、シリアライズ形状はテストで固定している。
- canonical state ではない。`runtime.json` と同じく runtime ディレクトリ直下の `genome.json` に置かれ、DB からは参照されない。Memory の rollback や derived lane の activation で変化しない。
- Genome が変わるのは `kamimusuhi-genome` CLI での明示操作だけ。対話中に runtime が gene を書き換える経路はない。
- runtime は gene を直接読まず、派生値 [`Phenotype`] だけを読む。gene→挙動の式は `Phenotype::from_genome` に集約されている。

## スキーマ (`kamimusuhi_core::genome`)

| 群 | gene |
|---|---|
| `temperament` | curiosity, caution, sociability, patience, expressiveness |
| `cognition` | reflection_depth, abstraction, precision, working_memory_span, associativity |
| `drives` | novelty_seeking, mastery, affiliation, autonomy, order |
| `adaptation` | plasticity, consolidation_bias, forgetting_rate, risk_tolerance, mutation_resistance |

メタデータ: `schema_version`, `genome_id` (`gen-<hex>`), `version` (世代。mutate で +1、clone では据え置き), `parent_id`, `hash` (`sha256:` 、gene 群のみの digest。ID は含まないので clone は親と同じ hash)。

gene は 1e-6 単位に量子化して保存する。JSON テキストを往復しても bit が変わらないので、メモリ上の hash とファイルから再計算した hash が一致する。読み込み時は範囲・schema・hash を検証し、手で編集して hash を更新し忘れたファイルは拒否する（runtime も起動を拒否する）。

## 操作

- `Genome::new_default()` / `with_id()`: 全 gene 0.5。
- `clone_genome()`: 同じ gene、新しい ID、`parent_id` 付き。
- `mutate(seed, rate)`: SplitMix64 による決定的変異。各 gene が確率 `rate` で `±0.15 × (1 − mutation_resistance)` の一様ステップを受け、clamp。同じ (genome, seed, rate) は常に同じ子を生む。
- `diff(a, b)`: 変化した gene と L2 距離、hash 一致。
- `phenotype()`: 下記の派生値。

## Phenotype と接続点

| 派生値 | 式 | 使用箇所 |
|---|---|---|
| `recall_breadth_scale` | `0.5 + curiosity` | `DialogueSession`: そのターンの `episodic_top_k` / `evidence_top_k` を倍率で拡縮（0 は 0 のまま、1 未満にはならない）。C0 lane の params 自体は書き換えない。 |
| `reflection_interval_scale` | `1.5 − reflection_depth` | `chat` REPL: `c0.reflection_interval_turns` を倍率で拡縮。0（手動のみ）は 0 のまま。 |
| `exploration_probability` | `0.05 + 0.6 × mean(curiosity, novelty_seeking)` | `DialogueSession`: 想起候補のうち baseline 外で判定が Uncertain のまま残ったものを workspace に採用するか（下記「探索分岐」）。 |
| `change_acceptance` | `mean(risk_tolerance, plasticity)` | 未接続 |
| `retention_bias` | `consolidation_bias − forgetting_rate` | 未接続 |
| `context_span_scale` | `0.5 + working_memory_span` | そのターンの会話履歴再生数を拡縮（Selection harness 追補を参照）。 |

既定 genome（全 0.5）では両倍率が 1.0 になり、既存挙動は変わらない。

## runtime との関係

- `init` は既定 genome を `genome.json` に書く。
- `open` は `genome.json` があれば読み（検証失敗は起動失敗）、なければ既定 genome をメモリ上で使い、書かない（旧ディレクトリ互換）。
- `inspect` の JSON に `genome`（ID・version・hash・phenotype）が加わる。boot trace にも `genome_id` / `genome_hash` を記録する。

## CLI

```bash
kamimusuhi-genome new <path> [--id <name>]
kamimusuhi-genome inspect <path>          # genome + genes + phenotype
kamimusuhi-genome phenotype <path>
kamimusuhi-genome clone <src> <dst> [--id <name>]
kamimusuhi-genome mutate <src> <dst> --seed <n> [--rate <0..1>] [--id <name>]
kamimusuhi-genome set <path> <group.gene> <value>
kamimusuhi-genome diff <a> <b>
```

`<path>` は `genome.json` か、それを含む runtime ディレクトリ。store は開かず writer epoch も取らないので、稼働中の runtime ディレクトリに対して安全に実行できる。変更は次回起動から反映される。

## 今回入れていないもの

- YAML（workspace に依存がないため JSON のみ）。
- 交叉 (crossover) や集団運用、Genome の DB 保存、GUI 表示。
- `exploration_probability` 等の未接続 phenotype の実挙動への配線。

## Selection harness (v0 追補)

`kamimusuhi-selection` は複数 genome を **同一条件**（同じ ID seed・固定 clock・fake Persona・fake reflector・同じ 10 ターンの台本）で実行し、gene → phenotype → 挙動指標の差を測る。fitness 関数や順位付けは持たない。目的は「遺伝子差が観測可能な挙動差を生む」ことの確認だけ。

```bash
kamimusuhi-selection run --out <dir> [--seed n] [--mutants n] [--rate r] \
    [--parent genome.json] [--genome <label>=<genome.json>]... \
    [--reflection-interval turns] [--turns n] [--json]
kamimusuhi-selection compare <dir | results.jsonl> [--json]
```

- 個体 = parent + `--mutants` 個の決定的 mutant（seed は `seed+1..`）+ `--genome` で与えた任意の genome。
- 個体ごとに `<dir>/individuals/<i>-<label>/` に runtime ディレクトリを作り、`<dir>/results.jsonl` に 1 行ずつ記録する: genome_id / hash / parent_id / version、phenotype、親と異なる gene、有効な内省間隔、完遂可否、指標、wall_ms、seed。
- 指標: 各ターンの `DialogueReply` に追加した `episodic_recalled` / `relationship_recalled` / `evidence_recalled` / `history_messages` の合計、cadence による reflection 回数とその activation 数、trace の `persona.invoked` / `resource.completed` 件数（LLM 呼び出し数）、tool 呼び出し数。
- 同じ seed と genome なら指標・hash・phenotype は完全一致する（テストで確認）。`wall_ms` だけは再現しない。

### 追加した配線

| phenotype | 式 | 接続 |
|---|---|---|
| `context_span_scale` | `0.5 + working_memory_span` | そのターンに Persona へ再生する会話履歴数（`retrieval.history_messages`）を拡縮。1 未満にはならない。履歴が短いほど古い発話は evidence 想起の対象になる。 |

`reflection_depth → 内省間隔` の判定は `c0::chat::reflection_due` に切り出し、REPL とハーネスが同じ関数を使う。

## 探索分岐 (exploration branch)

想起は 2 段階になっている。決定的な lexical baseline と、Jev（判定器）に提示される候補で、候補には baseline 外の「lexical 一致はないが同じ scope の新しい記録」が最大 2 件含まれる。判定器が `Uncertain` のまま残した候補は、従来は常に捨てられていた（baseline 維持）。

`exploration_probability` はこの 1 点に接続した。判定後、`Uncertain` かつ baseline 外の候補があるターンを **exploration opportunity** とし、seed 固定の乱数が確率を下回ったときだけそれらを `Relevant` に昇格して workspace に入れる。`Relevant` / `Irrelevant` と判定済みの候補は一切上書きしない。

- 乱数は turn ID だけから決まる（SplitMix64、genome には依存しない）。同じ seed の runtime では個体間で同じ乱数を引くので、分岐の差は確率差＝genome 差にのみ帰着する。
- 各ターンの `DialogueReply.exploration` と workspace context の `exploration` に、候補数・確率・乱数・採否・昇格数を記録する。
- 既定 genome では p = 0.35。以前の挙動（p = 0）とは、Uncertain な新規候補がある turn の 35% で workspace が広がる点だけが異なる。既存テストは全て通る。

### exploration scenario と結果

`kamimusuhi-selection run --scenario exploration --explorers --repeats N` は「探索しないと情報不足になる」台本を回す。2 ターン目の事実（焼き魚が一番のごちそう）が履歴窓から外れた後、9 ターン目で文字 bigram の重ならない言い回しで尋ねる。lexical baseline は必ず外し、探索分岐で昇格した場合だけ事実が workspace に届く。成功 = 9 ターン目の `recalled_evidence` / 記憶セクションに期待文字列が含まれること（履歴窓は判定から除外）。

seed 100 から 24 seed、`--reflection-interval 0`:

| 層 | 量 | parent (default) | low-exploration | high-exploration |
|---|---|---|---|---|
| gene | temperament.curiosity / drives.novelty_seeking | 0.5 / 0.5 | 0 / 0 | 1 / 1 |
| phenotype | exploration_probability | 0.35 | 0.05 | 0.65 |
| branch | exploration_opportunities (mean) | 3 | 3 | 3 |
| branch | exploration_attempted (mean) | 0.88 | 0.04 | 1.67 |
| branch | exploration_promoted (mean) | 1.75 | 0.04 | 3.29 |
| cost | context_records (mean) | 4.58 | 3 | 6.21 |
| cost | persona_calls (mean) | 10 | 10 | 10 |
| outcome | successes | 7/24 | 0/24 | 13/24 |

成功は探索を試みた run にしか現れず（success ⇒ attempted、全 run で確認）、発話回数（persona_calls）は変わらない。成功率とコストは別指標として JSONL に保存し、合成した fitness は計算しない。

## 世代交代実験 (evolution v0 追補)

### 前処理

- `runtime.json` に `genome_exploration: bool`（既定 false）を追加した。false のとき探索分岐は機会と乱数を記録するだけで昇格しない＝従来挙動。true にするのは operator か harness だけ（selection harness は個体ごとの runtime で true にする）。
- 探索指標名を単位付きに改めた: `opportunity_turns` / `exploration_turns_attempted` / `exploration_turns_skipped` / `candidates_promoted`。

### 仕組み (`kamimusuhi_runtime::evolution`, CLI `kamimusuhi-evolution`)

- 個体 = Genome + lineage（generation, origin = founder / survivor / offspring, parent_id, founder_id（世代 0 の祖先）, born_in_generation, mutation_seed）。
- **matched founders**: 世代 0 は `seed` と `population` だけから一様乱数で作る。環境が違っても同じ seed なら同じ founder 集団（テストで固定）。
- **2 つの seed panel** を分離する。
  - selection panel: `--repeats` 個の scenario seed。同一世代の全個体が同じ seed 集合を見る（common random numbers）。seed は世代ごとに変わる（`selection_seed(config, g)`、< 100000）。生存判定はこの panel だけで行う。
  - validation panel: `--validation-panel` 個の固定 seed（`--validation-seed` から連番、既定 900000 から 16）。全世代で同一、選択には一切使わない。世代 0 と最終世代の比較はこの panel で行う。
- 生存規則: selection panel で全 run 完遂、probe があれば success_rate ≥ 下限、resource budget 内。
- 次世代: 生存者を (seed, generation) の決定的 shuffle で並べ、先頭 population/2 をそのまま持ち越し、同じ順で round-robin に clone+mutate した子で埋める。配列位置は特権を持たない。生存者ゼロの世代は `selection_failed=true` で記録し、全員を親に子だけを作る（持ち越しなし）。交叉なし、fitness スコアなし。
- 出力: `lineage.jsonl`（個体×世代。genes / phenotype / lineage と、`selection` と `held_out` の panel 結果を別項目で保存。survived / failures / carried_over 付き）、`generations.jsonl`（selection 側の世代要約）、`adaptation.jsonl`（validation 側: success_rate、context_records、exploration_probability・explore_turns・promoted の分布、gene ごとの mean/variance、founder 系統数、distinct parents、unique hash）、`gen-<g>/` と `validation-<g>/` の harness 生データ、`config.json`。wall_ms 以外は完全再現。

環境:

| 名前 | scenario | 生存規則 |
|---|---|---|
| exploration-favoured | exploration（探索でしか事実に届かない） | success_rate ≥ 0.25 |
| distractor-heavy | distractor（baseline で答えられ、探索は無関係記録を混ぜ top-k=2 では正解を押し出す） | success_rate ≥ 0.75 かつ mean context_records ≤ 5 |
| baseline | baseline（probe なし） | 完遂のみ（drift） |

### 結果（matched founders: seed 1、8 個体、5 世代、selection panel 4 seed/世代、validation panel 16 seed 固定、mutation_rate 0.5）

3 環境とも世代 0 は同一の 8 founder（hash 一致を確認）。以下は **validation panel** での世代 0 → 4 の比較。selection panel の値は `generations.jsonl` に別途保存している。

| 環境 | validation success_rate | context_records | p_explore mean | p_explore var | explore_turns | promoted | gene variance | founder lineages | selection_failed 世代 |
|---|---|---|---|---|---|---|---|---|---|
| exploration-favoured | 0.14 → 0.38 | 5.07 → 6.54 | 0.311 → 0.492 | 0.0089 → 0.0002 | 0.85 → 1.51 | 1.76 → 3.02 | 0.057 → 0.002 | 8 → 1 | gen 0, 2 |
| distractor-heavy | 0.56 → 0.83 | 5.18 → 4.64 | 0.311 → 0.326 | 0.0089 → 0.0008 | 0.98 → 0.98 | 1.79 → 2.09 | 0.057 → 0.003 | 8 → 1 | なし |
| baseline | – | 5.02 → 6.52 | 0.311 → 0.129 | 0.0089 → 0.0003 | 0.98 → 0.53 | 1.88 → 1.45 | 0.057 → 0.002 | 8 → 1 | なし |

読み取り:

- **exploration-favoured**: p_explore の上昇（curiosity 0.46 → 0.84、novelty 0.41 → 0.64）と validation success の上昇（0.14 → 0.38）が同時に起き、探索回数・昇格数・context cost も増えた。ただし selection panel が 4 seed と小さいため世代 0 と 2 は誰も規則を満たさず `selection_failed=true`（fallback、全員が親）。実際の選択は世代 1（3 生存）と 3（1 生存）の 2 回で、最終世代は 1 系統（gen-g0-04）に収束した。
- **distractor-heavy**: p_explore はほぼ不変（0.311 → 0.326）だが validation success は 0.56 → 0.83、context_records は 5.18 → 4.64 に下がった。過剰探索した個体（context > 5 や top-k=2 で正解を押し出す個体）が規則で除かれた方向の変化で、selection_failed は発生していない。
- **baseline**: 全員生存で選択圧はないが、8 個体・持ち越し 4 の小集団では決定的 shuffle による持ち越しと round-robin の親選択だけで 4 世代のうちに 1 系統（gen-g0-00、p_explore 0.12）に固定した。p_explore の低下（0.311 → 0.129）は生存規則とは無関係で、どの founder が固定するかは shuffle seed 次第の drift。founder lineages の 8 → 1 は 3 環境に共通なので、系統数の減少だけでは選択と drift を区別できず、validation success と形質の同時変化で判断する必要がある。
- 3 環境とも persona_calls は 10 で不変、unique hash は各世代 8。

## 複数 replicate による selection と drift の分離 (batch v0 追補)

### panel の役割分担

| panel | seed | 用途 |
|---|---|---|
| selection | 世代ごとに変化（< 100000）、同一世代の全個体で共通 | 生存判定 |
| validation | 固定（既定 900000 から `--validation-panel` 個） | 世代ごとの適応計測。設計・閾値の調整に使ってよい（旧 held-out を改名） |
| final-test | 固定（既定 950000 から `--final-test-panel` 個） | 実験終了時に 1 回だけ、founder と最終世代を評価。選択にも調整にも使わない |

`final_test.json` に founder / 最終世代の panel 要約と個体別結果を保存する。

### batch (`kamimusuhi-evolution batch`, `kamimusuhi_runtime::evolution_batch`)

- replicate `r` は top-level seed `seed_base + r`。同一 replicate 内では 3 環境（exploration-favoured / distractor-heavy / baseline）が同じ founder を共有する（hash 一致を実行時に検証）。
- 各 replicate・各環境で世代 0 → 最終の差 Δ を validation panel（exploration_probability, curiosity, novelty_seeking, success_rate, context_records, exploration_turns_attempted, candidates_promoted, founder_lineages, mean_gene_variance, working_memory_span）と final-test panel（success_rate, context_records, exploration_turns_attempted, candidates_promoted）で保存し、`selection_failed` 世代数も記録する。
- paired difference `Δ(env) − Δ(baseline)` を replicate ごとに計算する。baseline には probe がないため success_rate は pair できず、raw Δ のみ報告する。
- `batch_summary.json` に量ごとの n / 値列 / mean / median / variance（不偏）/ seeded percentile bootstrap 95% CI / 符号数 / 期待符号と一致数を保存する。期待符号は exploration: p_explore・curiosity・novelty・success・explore_turns・promoted が +、distractor: context_records・promoted が −、success が +。それ以外は期待なし。
- replicate は独立なので thread 並列で実行する。thread 数は結果に影響しない（テストで 1 thread と 2 thread の一致を確認）。
- fitness スコアや環境間ランキングは作らない。生存条件は既存のもののみ。

### 結果（12 replicate、12 個体、6 世代、selection 8 seed/世代、validation 32 seed、final-test 32 seed、seed_base 1000）

selection_failed: exploration-favoured は 7 replicate で計 13 世代、distractor-heavy は 7 replicate で計 16 世代、baseline は 0。replicate 3 の exploration は 6 世代すべて失敗で、その Δ は純粋な drift。

validation panel、paired `Δ(env) − Δ(baseline)`（n=12、mean / median / 95% bootstrap CI / 符号 + − 0 / 期待符号一致）:

| 環境 | 量 | mean | median | 95% CI | + / − / 0 | 一致 |
|---|---|---|---|---|---|---|
| exploration-favoured | exploration_probability | +0.051 | +0.034 | [+0.008, +0.095] | 9 / 3 / 0 | 9/12 |
| exploration-favoured | curiosity | +0.148 | +0.118 | [−0.003, +0.289] | 9 / 3 / 0 | 9/12 |
| exploration-favoured | novelty_seeking | +0.022 | −0.003 | [−0.086, +0.138] | 6 / 6 / 0 | 6/12 |
| exploration-favoured | exploration_turns_attempted | +0.033 | −0.017 | [−0.204, +0.313] | 5 / 6 / 1 | 5/12 |
| exploration-favoured | candidates_promoted | −0.088 | −0.361 | [−0.578, +0.467] | 4 / 8 / 0 | 4/12 |
| distractor-heavy | exploration_probability | −0.060 | −0.051 | [−0.109, −0.015] | 3 / 9 / 0 | – |
| distractor-heavy | context_records | −1.241 | −1.516 | [−2.785, +0.402] | 4 / 8 / 0 | 8/12 |
| distractor-heavy | exploration_turns_attempted | −0.319 | −0.391 | [−0.569, −0.058] | 4 / 8 / 0 | – |
| distractor-heavy | candidates_promoted | −0.530 | −0.615 | [−0.961, −0.049] | 3 / 9 / 0 | 9/12 |

baseline の raw Δ: exploration_probability +0.006 [−0.029, +0.036]（7 / 5）、curiosity +0.009 [−0.092, +0.104]、context_records −0.213 [−1.289, +0.763]。方向性なし。founder lineages は 3 環境とも 12 → 約 2〜3 に減る（drift の共通成分）。

success_rate は baseline に probe がないため pair できない（raw Δ のみ）:

| panel | 環境 | Δsuccess mean | 95% CI | + / − / 0 |
|---|---|---|---|---|
| validation | exploration-favoured | +0.320 | [+0.242, +0.384] | 11 / 0 / 1 |
| validation | distractor-heavy | +0.288 | [+0.202, +0.380] | 12 / 0 / 0 |
| final-test | exploration-favoured | +0.305 | [+0.232, +0.362] | 11 / 0 / 1 |
| final-test | distractor-heavy | +0.276 | [+0.191, +0.369] | 12 / 0 / 0 |

final-test panel の paired 値は validation panel と同じ方向（distractor: context −1.237 [−2.768, +0.361] 8/12、promoted −0.545 [−0.964, −0.085] 9/12。exploration: explore_turns −0.044、promoted −0.233 で期待符号に一致せず）。

読み取り:

- **exploration-favoured の形質変化は drift との差として再現する。** paired Δexploration_probability は 9/12 で正、95% CI が 0 を含まない。curiosity も 9/12 で正（CI の下端は −0.003）。novelty_seeking には効果が見えない。success は 11/12 の replicate で上昇し、final-test panel でも同じ大きさで確認された（唯一の例外は 6 世代とも selection_failed だった replicate 3）。
- ただし **探索の実行回数（explore_turns, promoted）は drift 以上には増えていない**。success 上昇は p_explore だけでなく、`cognition.working_memory_span` が 0.5 付近に収束したことによる。exploration scenario では事実（2 ターン目）が「履歴窓のすぐ外側の最新 2 記録」に入るのは窓が 12 メッセージのとき（working_memory_span ∈ [0.458, 0.542)）だけで、この帯の個体の validation success は 0.455、帯外は 0.286、帯内かつ p ≥ 0.4 では 0.544（帯内 p < 0.3 は 0.237）。final の working_memory_span 分散は exploration で 0.076 → 0.011、baseline では 0.049 に留まる。つまり scenario は設計時の意図（探索確率）に加えて窓幅にも選択をかけており、これは harness 設計上の副作用として記録しておく。
- **distractor-heavy は過剰探索を抑制する方向に動く。** paired Δp_explore −0.060（CI が 0 を含まない、9/12）、explore_turns −0.319（CI 0 を含まず、8/12）、promoted −0.530（9/12）。context_records は 8/12 で減少するが CI は 0 をまたぐ（分散が大きい）。
- **baseline は drift のみ。** すべての形質で CI が 0 をまたぎ、符号もほぼ半々。

### 制約

- 12 thread 並列で約 55 分。batch 全体の同引数再実行は時間の都合で行っておらず、再現性はテスト（2 replicate、thread 1 と 2 で完全一致）と単一実験の再実行で確認している。
- selection panel が 8 seed のため、確率的な success では生存者ゼロの世代が珍しくない（exploration 7/12 replicate）。`selection_failed` 世代は drift として扱う。
